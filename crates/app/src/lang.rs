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

/// The language code the setting resolves to: `auto` → the OS language,
/// a shipped code as is, an OS-style tag (`pt_BR`) → the nearest shipped
/// code, any other short code as written (a player's own
/// `motif/lang/<code>.toml`; its missing keys read in English).
pub fn resolve(setting: &str) -> &'static str {
    let s = setting.trim();
    if s.is_empty() || s.eq_ignore_ascii_case(AUTO) {
        return detect();
    }
    if let Some((c, _)) = LANGS.iter().find(|(c, _)| c.eq_ignore_ascii_case(s)) {
        return c;
    }
    let near = nearest(s);
    let plain = s.len() <= 8 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if near == "en" && plain && !s.eq_ignore_ascii_case("en") {
        return own_code(s);
    }
    near
}

/// A code outside [`LANGS`], kept for the run (interned once per code).
fn own_code(s: &str) -> &'static str {
    static CODES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut v = CODES.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(c) = v.iter().find(|c| **c == s) {
        return c;
    }
    let c: &'static str = Box::leak(s.to_string().into_boxed_str());
    v.push(c);
    c
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
/// font with kana and kanji. Japanese runs of a text draw with it, sized
/// and centred on `fontN`'s capitals; Latin letters, digits and symbols
/// keep `fontN` (see [`crate::sprites::mixed_draw`]), so names and
/// numbers look as they do in English.
pub const JA_FONT_SLOT: i32 = 200;

/// The Japanese companion slot of `font` while the language is Japanese
/// and the theme has one (`has`).
pub fn ja_slot(font: i32, has: impl Fn(i32) -> bool) -> Option<i32> {
    let slot = font + JA_FONT_SLOT;
    (rugen_ui::lang::code().starts_with("ja") && has(slot)).then_some(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, ext, out);
            } else if p.extension().is_some_and(|x| x == ext) {
                out.push(p);
            }
        }
    }

    /// Every quoted `"section.key"` (or `"@section.key"`, spacing allowed
    /// after it) in the Rust sources and the screen TOMLs, plus the
    /// sections built at run time (`format!("pause.{id}")`), whose keys all
    /// count as used.
    fn used_keys(en: &rugen_ui::lang::Table) -> std::collections::BTreeSet<String> {
        let mut files = Vec::new();
        walk(&repo().join("crates/app/src"), "rs", &mut files);
        walk(&repo().join("crates/app/src"), "toml", &mut files);
        walk(&repo().join("motif/themes"), "toml", &mut files);
        let mut text = String::new();
        for f in files {
            text.push_str(&std::fs::read_to_string(&f).unwrap());
            text.push('\n');
        }
        let mut dynamic: Vec<String> = Vec::new();
        for (i, _) in text.match_indices(".{") {
            let head: String = text[..i]
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if text[..i].ends_with(&head) && text[..i - head.len()].ends_with('"') {
                dynamic.push(format!("{head}."));
            }
        }
        en.keys()
            .filter(|k| {
                text.contains(&format!("\"{k}\""))
                    || text.contains(&format!("\"@{k}\""))
                    || text.contains(&format!("\"@{k} "))
                    || dynamic.iter().any(|d| k.starts_with(d.as_str()))
            })
            .cloned()
            .collect()
    }

    /// Keys the sources ask for by name: `t("a.b")`, `tf("a.b"`, `tn(n, "a.b", "a.c")`
    /// and `"@a.b"` (comment lines skipped).
    fn asked_keys() -> Vec<String> {
        let mut files = Vec::new();
        walk(&repo().join("crates/app/src"), "rs", &mut files);
        walk(&repo().join("crates/app/src"), "toml", &mut files);
        walk(&repo().join("motif/themes"), "toml", &mut files);
        let key_char = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-';
        let mut keys = Vec::new();
        for f in files {
            for line in std::fs::read_to_string(&f).unwrap().lines() {
                let l = line.trim_start();
                if l.starts_with("//") || l.starts_with('#') {
                    continue;
                }
                let mut starts: Vec<usize> = l.match_indices("\"@").map(|(i, _)| i + 2).collect();
                for pat in ["t(\"", "tf(\"", "tn(", "lang::t(\"", "lang::tf(\""] {
                    for (i, _) in l.match_indices(pat) {
                        let before = l[..i].chars().next_back();
                        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                            continue;
                        }
                        let rest = &l[i + pat.len()..];
                        let end = rest.find(')').unwrap_or(rest.len());
                        for (j, _) in rest[..end].match_indices('"') {
                            starts.push(i + pat.len() + j + 1);
                        }
                        if pat.ends_with('"') {
                            starts.push(i + pat.len());
                        }
                    }
                }
                for st in starts {
                    let k: String = l[st..].chars().take_while(|c| key_char(*c)).collect();
                    if k.contains('.') && !k.ends_with('.') && !k.contains("..") {
                        keys.push(k);
                    }
                }
            }
        }
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    /// `rugen lang-check`: every language file has every key of en.toml
    /// with the same placeholders and no empty value, every key the sources
    /// ask for exists, and en.toml carries no key nobody uses.
    #[test]
    fn language_files_are_complete_and_keys_exist() {
        let dir = dir(&repo());
        let en = read_table(&dir, "en");
        assert!(!en.is_empty(), "motif/lang/en.toml is empty");
        let mut problems = Vec::new();
        for (code, _) in LANGS.iter().skip(1) {
            let path = dir.join(format!("{code}.toml"));
            if !path.exists() {
                problems.push(format!("{code}: no motif/lang/{code}.toml"));
                continue;
            }
            let table = read_table(&dir, code);
            for (k, v) in &en {
                match table.get(k) {
                    None => problems.push(format!("{code}: missing {k}")),
                    Some(t) if t.trim().is_empty() => problems.push(format!("{code}: empty {k}")),
                    Some(t) => {
                        let mut a = rugen_ui::lang::placeholders(v);
                        let mut b = rugen_ui::lang::placeholders(t);
                        a.sort_unstable();
                        b.sort_unstable();
                        if a != b {
                            problems.push(format!("{code}: placeholders of {k}: {t:?} vs {v:?}"));
                        }
                    }
                }
            }
            for k in table.keys() {
                if !en.contains_key(k) {
                    problems.push(format!("{code}: {k} is not in en.toml"));
                }
            }
        }
        let used = used_keys(&en);
        for k in asked_keys() {
            if !en.contains_key(&k) {
                problems.push(format!("en: {k} is used but not defined"));
            }
        }
        for k in en.keys() {
            if !used.contains(k) {
                problems.push(format!("en: {k} is defined but never used"));
            }
        }
        assert!(
            problems.is_empty(),
            "{} problems:\n  {}",
            problems.len(),
            problems.join("\n  ")
        );
    }

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
        assert_eq!(resolve("fr-CA"), "fr");
        assert_eq!(resolve("eo"), "eo");
        assert_eq!(resolve("en_GB"), "en");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
