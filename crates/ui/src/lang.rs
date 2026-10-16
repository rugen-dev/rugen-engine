//! Translations: one table of user-facing strings per language, keyed by
//! stable ids (`title.practice`, `options.update.check_now`), English as the
//! source and the fallback for every missing key.
//!
//! A screen TOML asks for a string with `text = "@title.practice"`; Rust
//! code calls [`t`] (a plain string) or [`tf`] (a format string with
//! `{name}` placeholders). The tables are set once at start and again when
//! the player picks another language or saves a language file
//! ([`set`]); every string handed out is `'static` (the tables are small
//! and leak on purpose), so settings tables and `json!` data keep working
//! with `&'static str`.
//!
//! Case follows the English source: a key whose English value is all
//! capitals (`"PRACTICE"`) draws in capitals in every Latin-script
//! language too (`"ENTRAÎNEMENT"`), however the translation file spells
//! it; Japanese has no case and keeps its text as written.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock, RwLock};

/// Flat `id -> text` table of one language.
pub type Table = HashMap<String, String>;

struct State {
    /// Language code (`en`, `pt-BR`, `ja`).
    code: &'static str,
    /// English source strings.
    en: HashMap<&'static str, &'static str>,
    /// The active language, already cased like the English source.
    cur: HashMap<&'static str, &'static str>,
}

fn state() -> &'static RwLock<State> {
    static S: OnceLock<RwLock<State>> = OnceLock::new();
    S.get_or_init(|| {
        RwLock::new(State {
            code: "en",
            en: HashMap::new(),
            cur: HashMap::new(),
        })
    })
}

/// Keys asked for and not found (each is logged once).
fn missing() -> &'static Mutex<HashSet<String>> {
    static M: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashSet::new()))
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// Letters only, none of them lowercase: the theme draws this in capitals.
pub fn is_shouty(s: &str) -> bool {
    let mut letters = false;
    for c in s.chars() {
        if c.is_lowercase() {
            return false;
        }
        letters |= c.is_alphabetic();
    }
    letters
}

/// Upper-case `s` for a Latin-script language: Turkish dots its capital I
/// (`i` → `İ`), everything else follows Unicode.
pub fn uppercase(s: &str, code: &str) -> String {
    if code.starts_with("tr") {
        s.chars()
            .map(|c| if c == 'i' { 'İ' } else { c })
            .collect::<String>()
            .to_uppercase()
    } else {
        s.to_uppercase()
    }
}

/// Scripts without letter case (the theme's capitals do not apply).
pub fn caseless(code: &str) -> bool {
    matches!(
        code.split(['-', '_']).next().unwrap_or(""),
        "ja" | "zh" | "ko"
    )
}

/// Install the English source table and the active language's table
/// (`lang` may be the English table itself). Values of `lang` are cased
/// like their English source (see the module docs).
pub fn set(code: &str, en: &Table, lang: &Table) {
    let code: &'static str = leak(code);
    let en_map: HashMap<&'static str, &'static str> =
        en.iter().map(|(k, v)| (leak(k), leak(v))).collect();
    let mut cur: HashMap<&'static str, &'static str> = HashMap::new();
    for (k, v) in lang {
        if v.trim().is_empty() {
            continue;
        }
        let shout = en.get(k).is_some_and(|e| is_shouty(e));
        let v = if shout && !caseless(code) {
            uppercase(v, code)
        } else {
            v.clone()
        };
        cur.insert(leak(k), leak(&v));
    }
    let mut s = state().write().unwrap_or_else(|p| p.into_inner());
    s.code = code;
    s.en = en_map;
    s.cur = cur;
    missing().lock().unwrap_or_else(|p| p.into_inner()).clear();
}

/// True once [`set`] installed a table.
pub fn is_loaded() -> bool {
    !state()
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .en
        .is_empty()
}

/// The active language code (`en` until [`set`] is called).
pub fn code() -> &'static str {
    state().read().unwrap_or_else(|p| p.into_inner()).code
}

/// True while the active language has no letter case (Japanese).
pub fn is_caseless() -> bool {
    caseless(code())
}

/// The string `key` names in the active language, else in English, else
/// the key itself (logged once).
pub fn t(key: &str) -> &'static str {
    let s = state().read().unwrap_or_else(|p| p.into_inner());
    if let Some(v) = s.cur.get(key).or_else(|| s.en.get(key)) {
        return v;
    }
    drop(s);
    let mut m = missing().lock().unwrap_or_else(|p| p.into_inner());
    if m.insert(key.to_string()) {
        log::warn!("lang: no string for {key:?}");
    }
    leak(key)
}

/// The English source of `key`, if the key exists.
pub fn source(key: &str) -> Option<&'static str> {
    let s = state().read().unwrap_or_else(|p| p.into_inner());
    s.en.get(key).copied()
}

/// True when `key` is in the English table.
pub fn has(key: &str) -> bool {
    source(key).is_some()
}

/// Replace `{name}` placeholders of `s` with `args` (unknown ones stay).
pub fn fill(s: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    if !s.contains('{') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 16);
    let mut rest = s;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) => {
                let name = &after[..j];
                match args.iter().find(|(n, _)| *n == name) {
                    Some((_, v)) => out.push_str(&v.to_string()),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// [`t`] with `{name}` placeholders filled from `args`.
pub fn tf(key: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    fill(t(key), args)
}

/// `@some.key` → that key, when the text is a translation reference (a
/// dotted id without spaces; `@name` glyphs have no dot).
pub fn key_of(text: &str) -> Option<&str> {
    let k = text.strip_prefix('@')?;
    (k.contains('.') && !k.contains(char::is_whitespace) && !k.contains('{')).then_some(k)
}

/// A screen string: `@key` resolved to its translation, anything else as is.
pub fn resolve(text: &str) -> std::borrow::Cow<'_, str> {
    match key_of(text) {
        Some(k) => std::borrow::Cow::Borrowed(t(k)),
        None => std::borrow::Cow::Borrowed(text),
    }
}

/// Flatten a parsed TOML document into dotted keys (`[title] practice =`
/// and `title.practice =` both become `title.practice`).
pub fn flatten(doc: &toml::Table) -> Table {
    fn walk(prefix: &str, t: &toml::Table, out: &mut Table) {
        for (k, v) in t {
            let key = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match v {
                toml::Value::Table(sub) => walk(&key, sub, out),
                toml::Value::String(s) => {
                    out.insert(key, s.clone());
                }
                other => {
                    out.insert(key, other.to_string());
                }
            }
        }
    }
    let mut out = Table::new();
    walk("", doc, &mut out);
    out
}

/// Parse a language file.
pub fn parse(text: &str) -> Result<Table, toml::de::Error> {
    let doc: toml::Table = toml::from_str(text)?;
    Ok(flatten(&doc))
}

/// Placeholder names (`{name}`) of a format string, in order.
pub fn placeholders(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find('{') {
        let after = &rest[i + 1..];
        let Some(j) = after.find('}') else { break };
        out.push(&after[..j]);
        rest = &after[j + 1..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(pairs: &[(&str, &str)]) -> Table {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn translations_follow_the_english_case_and_fall_back() {
        let en = table(&[
            ("title.practice", "PRACTICE"),
            ("title.hint", "Press enter"),
            ("title.only_en", "ONLY HERE"),
            ("fmt.by", "BY {name}"),
        ]);
        let fr = table(&[
            ("title.practice", "Entraînement"),
            ("title.hint", "Appuyez sur entrée"),
            ("fmt.by", "PAR {name}"),
        ]);
        set("fr", &en, &fr);
        assert_eq!(t("title.practice"), "ENTRAÎNEMENT");
        assert_eq!(t("title.hint"), "Appuyez sur entrée");
        assert_eq!(t("title.only_en"), "ONLY HERE");
        assert_eq!(t("nope.missing"), "nope.missing");
        assert_eq!(tf("fmt.by", &[("name", &"Zed")]), "PAR Zed");
        assert_eq!(code(), "fr");
        let ja = table(&[("title.practice", "トレーニング")]);
        set("ja", &en, &ja);
        assert_eq!(t("title.practice"), "トレーニング");
        assert!(is_caseless());
        set("en", &en, &en);
        assert_eq!(t("title.practice"), "PRACTICE");
    }

    #[test]
    fn turkish_capitals_dot_their_i() {
        assert_eq!(uppercase("izle", "tr"), "İZLE");
        assert_eq!(uppercase("izle", "en"), "IZLE");
    }

    #[test]
    fn keys_and_placeholders_parse() {
        assert_eq!(key_of("@title.practice"), Some("title.practice"));
        assert_eq!(key_of("@inf"), None);
        assert_eq!(key_of("@a.b c"), None);
        assert_eq!(placeholders("BY {name} IN {n} ROUNDS"), vec!["name", "n"]);
        assert_eq!(
            fill("{a} and {b} {c}", &[("a", &1), ("b", &"two")]),
            "1 and two {c}"
        );
        let t = parse("[title]\npractice = \"PRACTICE\"\nsub.line = \"x\"\n").unwrap();
        assert_eq!(t["title.practice"], "PRACTICE");
        assert_eq!(t["title.sub.line"], "x");
    }
}
