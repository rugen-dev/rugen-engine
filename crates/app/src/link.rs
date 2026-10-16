//! `rugen://` links: what the website hands the game.
//!
//! * `rugen://watch/<code>` opens the game straight on WATCH of that room on
//!   the official server ([`parse`]); the command line takes the link as its
//!   first argument (`rugen rugen://watch/K7MQX`) or after `--url`.
//! * The scheme has to be registered with the desktop once ([`register`]):
//!   Windows writes `HKCU\Software\Classes\rugen` (no administrator),
//!   Linux writes `~/.local/share/applications/rugen-url.desktop` with
//!   `MimeType=x-scheme-handler/rugen;` (the packaged `.desktop` carries the
//!   same line already) and tells `xdg-mime` about it. macOS needs an
//!   application bundle whose `Info.plist` declares `CFBundleURLTypes`; the
//!   bare binary in the tarball cannot register.
//!
//! The text each platform writes comes from a pure function so it can be
//! tested; only the thin `register` wrappers touch the system.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rugen_proto::room_code;

use crate::lang::t;

/// The scheme, without `:`.
pub const SCHEME: &str = "rugen";

/// Name of the user-level desktop entry Linux registers.
pub const DESKTOP_FILE: &str = "rugen-url.desktop";

/// What a link asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// Watch the room with this code (upper case) on the official server.
    Watch(String),
}

/// `rugen://watch/<code>` (any case; `rugen:watch/<code>` and a trailing
/// `/`, `?...` or `#...` are tolerated). Anything else says why not.
pub fn parse(url: &str) -> Result<Link, String> {
    let url = url.trim();
    let rest = url
        .get(..SCHEME.len() + 1)
        .filter(|head| head.eq_ignore_ascii_case(&format!("{SCHEME}:")))
        .map(|_| &url[SCHEME.len() + 1..])
        .ok_or_else(|| format!("not a {SCHEME}:// link: {url:?}"))?;
    let rest = rest.trim_start_matches('/');
    // nothing after `?` or `#` matters
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let mut parts = rest.split('/').filter(|p| !p.is_empty());
    let action = parts.next().unwrap_or("");
    match action.to_ascii_lowercase().as_str() {
        "watch" => {
            let code = room_code::normalize(parts.next().unwrap_or(""));
            if code.is_empty() {
                return Err(format!("{SCHEME}://watch/ needs a room code"));
            }
            if parts.next().is_some() {
                return Err(format!("{SCHEME}://watch/<code> takes one code: {url:?}"));
            }
            if code.len() > 8 || !code.chars().all(room_code::valid_char) {
                return Err(format!("{code:?} is not a room code"));
            }
            Ok(Link::Watch(code))
        }
        "" => Err(format!(
            "{SCHEME}:// needs an action, like {SCHEME}://watch/<code>"
        )),
        other => Err(format!("unknown {SCHEME}:// action {other:?}")),
    }
}

// ---- Linux: a user desktop entry

/// A path as the `Exec=` key of a desktop entry wants it: quoted when it
/// holds anything outside plain path characters, with the entry's own
/// escapes inside the quotes.
fn exec_quote(exe: &Path) -> String {
    let s = exe.to_string_lossy();
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+');
    if !s.is_empty() && s.chars().all(plain) {
        return s.into_owned();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The desktop entry that routes `rugen://` links to `exe` (hidden from the
/// application menus: the packaged `rugen.desktop` is the visible one).
pub fn desktop_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=rugen (links)\n\
         Comment=Opens rugen:// links from rugen.dev\n\
         Exec={} %u\n\
         Icon=rugen\n\
         Terminal=false\n\
         NoDisplay=true\n\
         StartupNotify=false\n\
         Categories=Game;\n\
         MimeType=x-scheme-handler/{SCHEME};\n",
        exec_quote(exe)
    )
}

/// `~/.local/share/applications` (`$XDG_DATA_HOME/applications`).
#[cfg(target_os = "linux")]
fn applications_dir() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(data.join("applications"))
}

// ---- Windows: the per-user registry

/// The `reg add` commands (arguments after `reg`) that register the scheme
/// for the current user: the class, its `URL Protocol` marker, an icon and
/// the open command handing the link to `exe`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn reg_commands(exe: &Path) -> Vec<Vec<String>> {
    let exe = exe.to_string_lossy();
    let key = format!("HKCU\\Software\\Classes\\{SCHEME}");
    let strs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    vec![
        strs(&["add", &key, "/ve", "/d", "URL:rugen link", "/f"]),
        strs(&["add", &key, "/v", "URL Protocol", "/d", "", "/f"]),
        strs(&[
            "add",
            &format!("{key}\\DefaultIcon"),
            "/ve",
            "/d",
            &format!("\"{exe}\",0"),
            "/f",
        ]),
        strs(&[
            "add",
            &format!("{key}\\shell\\open\\command"),
            "/ve",
            "/d",
            &format!("\"{exe}\" \"%1\""),
            "/f",
        ]),
    ]
}

// ---- doing it

/// The last outcome of [`register`] this session, for the OPTIONS row.
static STATUS: Mutex<String> = Mutex::new(String::new());

pub fn status() -> String {
    STATUS.lock().map(|s| s.clone()).unwrap_or_default()
}

fn set_status(s: &str) {
    if let Ok(mut st) = STATUS.lock() {
        *st = s.to_string();
    }
}

/// Register the scheme for the current user with this very executable.
/// Returns a short line for the screen; the status row keeps it.
pub fn register() -> Result<String, String> {
    let r = register_with(&std::env::current_exe().map_err(|e| format!("where am I? {e}"))?);
    match &r {
        Ok(line) => {
            log::info!("{SCHEME}:// links: {line}");
            set_status(t("link.done"));
        }
        Err(e) => {
            log::warn!("{SCHEME}:// links: {e}");
            set_status(t("link.failed"));
        }
    }
    r
}

#[cfg(target_os = "linux")]
fn register_with(exe: &Path) -> Result<String, String> {
    let dir = applications_dir().ok_or("no home directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join(DESKTOP_FILE);
    std::fs::write(&file, desktop_entry(exe)).map_err(|e| format!("{}: {e}", file.display()))?;
    // tell the desktop (both are optional helpers: the file alone works
    // for desktops that read the MimeType line themselves)
    let run = |cmd: &str, args: &[&str]| match std::process::Command::new(cmd).args(args).status() {
        Ok(s) if s.success() => true,
        Ok(s) => {
            log::warn!("{cmd}: {s}");
            false
        }
        Err(e) => {
            log::info!("{cmd}: {e}");
            false
        }
    };
    let told = run(
        "xdg-mime",
        &[
            "default",
            DESKTOP_FILE,
            &format!("x-scheme-handler/{SCHEME}"),
        ],
    );
    run("update-desktop-database", &[dir.to_string_lossy().as_ref()]);
    Ok(if told {
        format!("{SCHEME}:// LINKS OPEN THIS GAME ({})", file.display())
    } else {
        format!(
            "WROTE {} (XDG-MIME NOT FOUND: SOME DESKTOPS NEED A LOGOUT)",
            file.display()
        )
    })
}

#[cfg(windows)]
fn register_with(exe: &Path) -> Result<String, String> {
    for args in reg_commands(exe) {
        let out = std::process::Command::new("reg")
            .args(&args)
            .output()
            .map_err(|e| format!("reg: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "reg {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }
    Ok(format!(
        "{SCHEME}:// LINKS OPEN THIS GAME (HKCU\\SOFTWARE\\CLASSES\\{SCHEME})"
    ))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn register_with(_exe: &Path) -> Result<String, String> {
    Err(format!(
        "{SCHEME}:// LINKS NEED AN APPLICATION BUNDLE ON THIS SYSTEM (NOT THIS BUILD)"
    ))
}

/// Whether this system can register the scheme at all (the OPTIONS row).
pub fn supported() -> bool {
    cfg!(any(target_os = "linux", windows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_links_parse_in_any_case_and_shape() {
        assert_eq!(
            parse("rugen://watch/K7MQX"),
            Ok(Link::Watch("K7MQX".into()))
        );
        assert_eq!(
            parse("RUGEN://WATCH/k7mqx/"),
            Ok(Link::Watch("K7MQX".into()))
        );
        assert_eq!(parse("rugen:watch/K7MQX"), Ok(Link::Watch("K7MQX".into())));
        assert_eq!(
            parse("  rugen://watch/K7MQX?from=site#x "),
            Ok(Link::Watch("K7MQX".into()))
        );
    }

    #[test]
    fn bad_links_say_why() {
        assert!(parse("https://rugen.dev/watch/K7MQX").is_err());
        assert!(parse("rugen://").unwrap_err().contains("action"));
        assert!(parse("rugen://watch").unwrap_err().contains("room code"));
        assert!(parse("rugen://watch/").unwrap_err().contains("room code"));
        assert!(parse("rugen://watch/K7MQX/extra").is_err());
        // 0, O, 1 and I are never in a code; neither is punctuation
        assert!(parse("rugen://watch/K0MQX").is_err());
        assert!(parse("rugen://watch/K7M-X").is_err());
        assert!(parse("rugen://watch/TOOLONGCODE").is_err());
        assert!(parse("rugen://play/K7MQX").unwrap_err().contains("play"));
    }

    #[test]
    fn desktop_entry_routes_the_scheme_to_the_binary() {
        let d = desktop_entry(Path::new("/opt/rugen/rugen"));
        assert!(d.starts_with("[Desktop Entry]\nType=Application\n"));
        assert!(d.contains("\nExec=/opt/rugen/rugen %u\n"));
        assert!(d.contains("\nMimeType=x-scheme-handler/rugen;\n"));
        assert!(d.contains("\nNoDisplay=true\n"));
        assert!(d.ends_with('\n'));
        // paths with spaces or reserved characters are quoted and escaped
        let d = desktop_entry(Path::new("/home/me/My Games/rugen"));
        assert!(d.contains("\nExec=\"/home/me/My Games/rugen\" %u\n"));
        let d = desktop_entry(Path::new("/tmp/a\"b$c/rugen"));
        assert!(d.contains("\nExec=\"/tmp/a\\\"b\\$c/rugen\" %u\n"));
    }

    #[test]
    fn registry_commands_write_the_per_user_class() {
        let cmds = reg_commands(Path::new("C:\\Games\\rugen\\rugen.exe"));
        assert_eq!(cmds.len(), 4);
        for c in &cmds {
            assert_eq!(c[0], "add");
            assert!(c[1].starts_with("HKCU\\Software\\Classes\\rugen"));
            assert_eq!(c.last().unwrap(), "/f");
        }
        assert_eq!(cmds[0][1], "HKCU\\Software\\Classes\\rugen");
        assert!(cmds[0].contains(&"URL:rugen link".to_string()));
        assert_eq!(cmds[1][2..5], ["/v", "URL Protocol", "/d"]);
        assert_eq!(cmds[2][1], "HKCU\\Software\\Classes\\rugen\\DefaultIcon");
        assert_eq!(cmds[2][4], "\"C:\\Games\\rugen\\rugen.exe\",0");
        assert_eq!(
            cmds[3][1],
            "HKCU\\Software\\Classes\\rugen\\shell\\open\\command"
        );
        assert_eq!(cmds[3][4], "\"C:\\Games\\rugen\\rugen.exe\" \"%1\"");
    }
}
