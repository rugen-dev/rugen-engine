//! The game's languages: `motif/lang/<code>.toml` files (English is the
//! source), the OS locale, and the `[interface] lang` setting. The strings
//! themselves live in [`rugen_ui::lang`] (`t`, `tf`, `@key` in screens).

use std::path::{Path, PathBuf};

/// The string `key` names in the active language (English when the
/// language lacks it; the key itself when nobody has it).
pub fn t(key: &str) -> &'static str {
    #[cfg(test)]
    test_tables();
    rugen_ui::lang::t(key)
}

/// [`t`] with `{name}` placeholders filled from `args`:
/// `tf("select.by", &[("name", &author)])`.
pub fn tf(key: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    #[cfg(test)]
    test_tables();
    rugen_ui::lang::tf(key, args)
}

/// `n` and the singular or plural of a counted thing:
/// `tn(2, "online.room", "online.rooms")` → `2 ROOMS`.
pub fn tn(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", t(if n == 1 { one } else { many }))
}

/// Tests read the English strings of the repository's `motif/lang/en.toml`
/// (loaded once; a test switching languages must put English back).
#[cfg(test)]
pub fn test_tables() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        if !rugen_ui::lang::is_loaded() {
            load(&assets, "en");
        }
    });
}

/// `(code, name in its own language)`, in picker order. English first,
/// then by code.
pub const LANGS: [(&str, &str); 11] = [
    ("en", "English"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("fil", "Filipino"),
    ("fr", "Français"),
    ("id", "Bahasa Indonesia"),
    ("it", "Italiano"),
    ("ja", "日本語"),
    ("pl", "Polski"),
    ("pt-BR", "Português (Brasil)"),
    ("tr", "Türkçe"),
];

/// `[interface] lang` value meaning "the OS language".
pub const AUTO: &str = "auto";

/// The languages' codes.
pub fn codes() -> impl Iterator<Item = &'static str> {
    LANGS.iter().map(|(c, _)| *c)
}

/// A language's name in itself (`English` for an unknown code).
pub fn label(code: &str) -> &'static str {
    LANGS
        .iter()
        .find(|(c, _)| c.eq_ignore_ascii_case(code))
        .map_or(LANGS[0].1, |(_, n)| n)
}

/// Index of `setting` in the picker: 0 = AUTO, then [`LANGS`] order.
pub fn picker_index(setting: &str) -> usize {
    let s = setting.trim();
    if s.is_empty() || s.eq_ignore_ascii_case(AUTO) {
        return 0;
    }
    LANGS
        .iter()
        .position(|(c, _)| c.eq_ignore_ascii_case(s))
        .map_or(0, |i| i + 1)
}

/// The setting the picker index stands for.
pub fn picker_value(index: usize) -> &'static str {
    match index.checked_sub(1).and_then(|i| LANGS.get(i)) {
        Some((c, _)) => c,
        None => AUTO,
    }
}

/// The directory of the language files: `motif/lang/` next to the themes.
pub fn dir(assets: &Path) -> PathBuf {
    assets.join("motif").join("lang")
}

/// Map an OS locale tag (`pt_BR.UTF-8`, `fr-CA`, `ja_JP`, `fil-PH`) to the
/// nearest language code; `en` when nothing matches.
pub fn nearest(tag: &str) -> &'static str {
    let tag = tag.trim();
    let tag = tag.split(['.', '@']).next().unwrap_or("");
    let mut parts = tag.split(['-', '_']);
    let base = parts.next().unwrap_or("").to_ascii_lowercase();
    if base.is_empty() || base == "c" || base == "posix" {
        return "en";
    }
    let region = parts.next().unwrap_or("").to_ascii_uppercase();
    // exact `base-REGION` first (pt-BR), then the base language, then
    // Brazilian Portuguese for every Portuguese (the only pt we ship)
    let full = format!("{base}-{region}");
    if let Some((c, _)) = LANGS.iter().find(|(c, _)| c.eq_ignore_ascii_case(&full)) {
        return c;
    }
    if let Some((c, _)) = LANGS.iter().find(|(c, _)| c.eq_ignore_ascii_case(&base)) {
        return c;
    }
    match base.as_str() {
        "pt" => "pt-BR",
        "tl" => "fil",
        _ => "en",
    }
}

/// The OS language: `LC_ALL`, `LC_MESSAGES`, `LANG` (and `LANGUAGE`'s first
/// entry) on Unix, the user default locale on Windows, `AppleLocale` /
/// `AppleLanguages` on macOS. `en` when nothing is set.
pub fn detect() -> &'static str {
    if let Some(tag) = os_locale() {
        return nearest(&tag);
    }
    "en"
}

#[cfg(target_os = "windows")]
fn os_locale() -> Option<String> {
    // GetUserDefaultLocaleName without a windows crate: PowerShell is heavy,
    // the environment first (MSYS / Git Bash set LANG), then the registry
    // value Windows keeps for it.
    if let Some(t) = env_locale() {
        return Some(t);
    }
    let out = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Control Panel\International",
            "/v",
            "LocaleName",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find(|l| l.contains("LocaleName"))
        .and_then(|l| l.split_whitespace().last())
        .map(str::to_string)
}

#[cfg(target_os = "macos")]
fn os_locale() -> Option<String> {
    if let Some(t) = env_locale() {
        return Some(t);
    }
    for key in ["AppleLocale", "AppleLanguages"] {
        let out = std::process::Command::new("defaults")
            .args(["read", "-g", key])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        // AppleLanguages prints a list: `(\n    "fr-FR",\n ...`
        let first = text
            .lines()
            .map(|l| l.trim().trim_matches(['(', ')', '"', ',']).trim())
            .find(|l| !l.is_empty());
        if let Some(t) = first {
            return Some(t.to_string());
        }
    }
    None
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn os_locale() -> Option<String> {
    env_locale()
}

fn env_locale() -> Option<String> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(key)
            && !v.trim().is_empty()
        {
            return Some(v);
        }
    }
    std::env::var("LANGUAGE")
        .ok()
        .and_then(|v| v.split(':').next().map(str::to_string))
        .filter(|v| !v.trim().is_empty())
}

/// The language code the setting resolves to (`auto` → the OS language).
pub fn resolve(setting: &str) -> &'static str {
    let s = setting.trim();
    if s.is_empty() || s.eq_ignore_ascii_case(AUTO) {
        return detect();
    }
    LANGS
        .iter()
        .find(|(c, _)| c.eq_ignore_ascii_case(s))
        .map_or_else(|| nearest(s), |(c, _)| c)
}

fn read_table(dir: &Path, code: &str) -> rugen_ui::lang::Table {
    let path = dir.join(format!("{code}.toml"));
    match std::fs::read_to_string(&path) {
        Ok(text) => match rugen_ui::lang::parse(&text) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("lang: {}: {e}", path.display());
                Default::default()
            }
        },
        Err(e) => {
            log::warn!("lang: {}: {e}", path.display());
            Default::default()
        }
    }
}

/// Load `en.toml` and the resolved language's file from `motif/lang/` and
/// make them the active strings. Returns the code in use.
pub fn load(assets: &Path, setting: &str) -> &'static str {
    let code = resolve(setting);
    let dir = dir(assets);
    let en = read_table(&dir, "en");
    let cur = if code == "en" {
        en.clone()
    } else {
        read_table(&dir, code)
    };
    rugen_ui::lang::set(code, &en, &cur);
    log::info!("lang: {code} ({} strings)", cur.len());
    code
}

/// `system.def` `fontN.ja` loads into slot `N + JA_FONT_SLOT`: the pixel
/// font with kana and kanji, drawn in place of `fontN` while the language
/// is Japanese (whatever the scale: Latin faces have no Japanese glyphs).
pub const JA_FONT_SLOT: i32 = 200;

/// The suffix of the font key the active language asks for (`ja`), if any.
pub fn font_suffix() -> Option<&'static str> {
    rugen_ui::lang::code().starts_with("ja").then_some("ja")
}

/// The font slot a screen's `font` draws with in the active language:
/// the `.ja` variant while Japanese is on and the theme has one (`has`).
pub fn font_slot(font: i32, has: impl Fn(i32) -> bool) -> i32 {
    let slot = font + JA_FONT_SLOT;
    if font_suffix().is_some() && has(slot) {
        slot
    } else {
        font
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_map_to_the_nearest_language() {
        assert_eq!(nearest("pt_BR.UTF-8"), "pt-BR");
        assert_eq!(nearest("pt_PT"), "pt-BR");
        assert_eq!(nearest("fr-CA"), "fr");
        assert_eq!(nearest("ja_JP.UTF-8"), "ja");
        assert_eq!(nearest("fil-PH"), "fil");
        assert_eq!(nearest("tl"), "fil");
        assert_eq!(nearest("C"), "en");
        assert_eq!(nearest(""), "en");
        assert_eq!(nearest("zh_CN"), "en");
        assert_eq!(nearest("de_AT@euro"), "de");
    }

    #[test]
    fn picker_round_trips() {
        assert_eq!(picker_index("auto"), 0);
        assert_eq!(picker_value(0), AUTO);
        for (i, (c, _)) in LANGS.iter().enumerate() {
            assert_eq!(picker_index(c), i + 1);
            assert_eq!(picker_value(i + 1), *c);
        }
        assert_eq!(label("ja"), "日本語");
        assert_eq!(label("xx"), "English");
    }

    /// (The global tables are left alone: other tests read them.)
    #[test]
    fn language_files_read_and_flatten() {
        let tmp = std::env::temp_dir().join(format!("rugen-lang-{}", std::process::id()));
        let d = tmp.join("motif").join("lang");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("fr.toml"), "[x]\na = \"Salut\"\nsub.b = \"c\"\n").unwrap();
        let t = read_table(&dir(&tmp), "fr");
        assert_eq!(t["x.a"], "Salut");
        assert_eq!(t["x.sub.b"], "c");
        assert!(read_table(&dir(&tmp), "xx").is_empty());
        assert_eq!(resolve("fr"), "fr");
        assert_eq!(resolve("pt_BR"), "pt-BR");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
