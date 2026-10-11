//! `.def` / INI-style files: character and stage definitions, screenpacks,
//! `.cns` constants sections, `.cmd` command blocks (structure only).
//!
//! Semantics follow MUGEN: `;` comments, `[Section rest]` headers, keys are
//! lowercased and end at the first `=`, space or tab, the first occurrence
//! of a key in a section wins, sections may repeat (`[BG 0]`, `[BG 1]`).

use indexmap::IndexMap;

use crate::text::{atof, atoi, section, strip_comment};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    /// Lowercased name up to the first space, e.g. `info`, `bg`, `begin`, `state`.
    pub name: String,
    /// Rest of the header, original case, e.g. `0` in `[BG 0]`, `Action 200` in `[Begin Action 200]`.
    pub rest: String,
    /// 1-based line of the header.
    pub line: usize,
    pub entries: IndexMap<String, String>,
}

impl Section {
    pub fn raw(&self, key: &str) -> Option<&str> {
        self.entries
            .get(&key.to_ascii_lowercase())
            .map(String::as_str)
    }
    /// String value with surrounding quotes removed (MUGEN warns when absent; we accept both).
    pub fn str(&self, key: &str) -> Option<&str> {
        self.raw(key).map(|v| {
            v.strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(v)
        })
    }
    pub fn i32(&self, key: &str) -> Option<i32> {
        self.raw(key).filter(|v| !v.is_empty()).map(atoi)
    }
    pub fn f32(&self, key: &str) -> Option<f32> {
        self.raw(key).and_then(atof)
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        self.i32(key).map(|v| v != 0)
    }
    /// Comma-separated ints; missing or empty items read as 0 like MUGEN's atoi.
    pub fn i32s(&self, key: &str) -> Option<Vec<i32>> {
        self.raw(key).map(|v| v.split(',').map(atoi).collect())
    }
    pub fn f32s(&self, key: &str) -> Option<Vec<f32>> {
        self.raw(key)
            .map(|v| v.split(',').map(|s| atof(s).unwrap_or(0.0)).collect())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Def {
    pub sections: Vec<Section>,
}

impl Def {
    pub fn parse(text: &str) -> Self {
        let mut def = Def::default();
        let mut cur: Option<Section> = None;
        for (i, raw) in text.lines().enumerate() {
            if let Some((name, rest)) = section(raw) {
                if let Some(s) = cur.take() {
                    def.sections.push(s);
                }
                cur = Some(Section {
                    name: name.trim_end().to_string(),
                    rest: rest.to_string(),
                    line: i + 1,
                    entries: IndexMap::new(),
                });
                continue;
            }
            let Some(s) = cur.as_mut() else { continue };
            let line = strip_comment(raw);
            let Some(ia) = line.find(['=', ' ', '\t']) else {
                continue;
            };
            if ia == 0 {
                continue;
            }
            let key = line[..ia].to_ascii_lowercase();
            let value = line.find('=').map(|e| line[e + 1..].trim()).unwrap_or("");
            s.entries.entry(key).or_insert_with(|| value.to_string());
        }
        if let Some(s) = cur {
            def.sections.push(s);
        }
        def
    }

    pub fn open(path: impl AsRef<std::path::Path>) -> crate::Result<Self> {
        Ok(Self::parse(&crate::text::decode(&std::fs::read(path)?)))
    }

    /// First section with this name (case-insensitive).
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }

    pub fn sections_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Section> + 'a {
        self.sections
            .iter()
            .filter(move |s| s.name.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::reference;

    #[test]
    fn basics() {
        let d = Def::parse(
            "; c\n[Info]\nname = \"KFM\" ;x\nlocalcoord = 320,240\nname = dup\n[BG 0]\ntype=normal\nstart = 0, -10\n[BG 1]\ntype = parallax\n",
        );
        let info = d.section("info").unwrap();
        assert_eq!(info.str("name"), Some("KFM"));
        assert_eq!(info.raw("name"), Some("\"KFM\""));
        assert_eq!(info.i32s("localcoord"), Some(vec![320, 240]));
        assert_eq!(d.sections_named("bg").count(), 2);
        let bg0 = d.section("bg").unwrap();
        assert_eq!(bg0.rest, "0");
        assert_eq!(bg0.raw("type"), Some("normal"));
        assert_eq!(bg0.i32s("start"), Some(vec![0, -10]));
    }

    #[test]
    fn key_ends_at_space_or_tab() {
        let d = Def::parse("[x]\nfoo bar = 1\nbaz\t= 2\n=3\nnovalue\n");
        let s = d.section("x").unwrap();
        assert_eq!(s.raw("foo"), Some("1"));
        assert_eq!(s.raw("baz"), Some("2"));
        assert_eq!(s.entries.len(), 2);
    }

    #[test]
    fn kfm_def() {
        let Some(p) = reference("mugen-1.1b1/chars/kfm/kfm.def") else {
            return;
        };
        let d = Def::open(p).unwrap();
        let info = d.section("Info").unwrap();
        assert_eq!(info.str("name"), Some("Kung Fu Man"));
        assert_eq!(info.f32("mugenversion"), Some(1.0));
        assert_eq!(info.i32s("pal.defaults"), Some(vec![1, 2, 3, 4]));
        let files = d.section("files").unwrap();
        assert_eq!(files.raw("sprite"), Some("kfm.sff"));
        assert_eq!(files.raw("stcommon"), Some("common1.cns"));
    }

    #[test]
    fn stage0_def() {
        let Some(p) = reference("mugen-1.1b1/stages/stage0.def") else {
            return;
        };
        let d = Def::open(p).unwrap();
        let cam = d.section("camera").unwrap();
        assert_eq!(cam.i32("boundleft"), Some(-125));
        assert_eq!(cam.f32("verticalfollow"), Some(0.2));
        assert!(d.sections_named("bg").count() >= 2);
    }

    #[test]
    fn all_reference_defs_parse() {
        let Some(root) = reference("") else { return };
        let mut n = 0;
        for dir in ["mugen-1.1b1", "ikemen-go-v1.0.0"] {
            for p in crate::fixtures::walk(&root.join(dir)) {
                if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("def")) {
                    let d = Def::open(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                    assert!(!d.sections.is_empty(), "{} has no sections", p.display());
                    n += 1;
                }
            }
        }
        assert!(n >= 50, "found only {n} def files");
    }
}
