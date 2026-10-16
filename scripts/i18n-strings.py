#!/usr/bin/env python3
"""User-facing strings of the rugen UI: list them, move the screen TOMLs'
literals into `motif/lang/en.toml`, and check the language files.

  scripts/i18n-strings.py list            every candidate string (TOML sources
                                          and Rust literals that look like UI
                                          text), one per line: file:line<TAB>text
  scripts/i18n-strings.py convert-toml    rewrite the screen TOML sources so each
                                          literal reads `@<key>`, adding the keys
                                          to motif/lang/en.toml (idempotent)
  scripts/i18n-strings.py check           every motif/lang/<code>.toml has every
                                          key of en.toml, placeholders match, no
                                          empty values (exit 1 on a problem)
  scripts/i18n-strings.py kanji           the kana and kanji ja.toml uses (the
                                          glyph set of the Japanese font)

Run from the repository root. The Rust sources keep their keys by hand
(`crate::lang::t("key")`): `list` only points at them.
"""

import os
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LANG_DIR = ROOT / "motif" / "lang"
APP = ROOT / "crates" / "app" / "src"

# screen TOML sources (the motif copies are generated from these)
TOML_SOURCES = sorted(
    list((APP / "inkscreens").glob("*.toml")) + list((APP / "dojoscreens").glob("*.toml"))
)
# Rust files holding TOML text (raw string constants)
RUST_TOML_SOURCES = [APP / "settings_gen.rs"]

TEXT_KEYS = ("text", "placeholder", "tip")
LINE_RE = re.compile(r'^(\s*)(' + "|".join(TEXT_KEYS) + r') = "((?:[^"\\]|\\.)*)"(\s*(#.*)?)$')
LETTERS_RE = re.compile(r"[^\W\d_]{2}", re.UNICODE)
BIND_RE = re.compile(r"\{[^{}]*\}")

# Rust literal heuristics: two capitals in a row or a sentence with a space,
# not on a line that logs, bails, formats a path or names an action/id.
RUST_SKIP_LINE = re.compile(
    r"log::|anyhow!|bail!|context\(|println!|eprintln!|#\[|//|assert|\.join\(|Path::|env::var|"
    r'"press"|"id"|"glyph"|"image"|starts_with|strip_prefix|ends_with|== "|!= "|match |=> "'
)
RUST_LIT = re.compile(r'"((?:[^"\\]|\\.)*)"')
RUST_TEXTY = re.compile(r"(?:[A-Z]{2,}|[A-Za-z]+ [A-Za-z]+)")


def user_facing(value: str) -> bool:
    """A TOML value worth translating: holds two letters in a row outside
    its `{binds}`, is not an `@key` already, not a bind-only template."""
    if value.startswith("@") or "{{" in value:
        return False
    stripped = BIND_RE.sub("", value)
    if not LETTERS_RE.search(stripped):
        return False
    if re.fullmatch(r"\s*[Pp]\d\s*", stripped):
        return False
    return True


def slug(value: str, words: int = 6) -> str:
    """`BY {card1.author}` → `by_author`; sentences keep their first words."""

    def bind_word(m):
        return m.group(0).strip("{}").split(".")[-1]

    s = BIND_RE.sub(bind_word, value)
    s = s.replace("rugen://", "links").replace("rugen.dev", "site")
    s = re.sub(r"[^\w]+", "_", s, flags=re.UNICODE).strip("_").lower()
    parts = [p for p in s.split("_") if p]
    if not parts:
        return "text"
    s = "_".join(parts[:words])
    return s[:48].rstrip("_") or "text"


def toml_literals():
    """(path, line_no, indent, key, value, trailer) for every literal line."""
    for p in TOML_SOURCES + RUST_TOML_SOURCES:
        for i, line in enumerate(p.read_text(encoding="utf-8").splitlines(), 1):
            m = LINE_RE.match(line)
            if not m:
                continue
            value = m.group(3)
            if not user_facing(value):
                continue
            yield p, i, m.group(1), m.group(2), value, m.group(4)


def rust_candidates():
    for p in sorted(APP.glob("*.rs")):
        if p.name in ("fontgen.rs", "inkgen.rs", "dojogen.rs", "inkscreens.rs", "stagegen.rs"):
            continue
        for i, line in enumerate(p.read_text(encoding="utf-8").splitlines(), 1):
            if RUST_SKIP_LINE.search(line):
                continue
            for m in RUST_LIT.finditer(line):
                v = m.group(1)
                if v.startswith("@") or not RUST_TEXTY.search(v):
                    continue
                if "/" in v and " " not in v:
                    continue
                yield p, i, v


def namespace(path: Path) -> str:
    if path.suffix == ".rs":
        return "options"  # settings_gen.rs: options + controls pages
    return path.stem


def cmd_list():
    for p, i, _ind, _k, v, _t in toml_literals():
        print(f"{p.relative_to(ROOT)}:{i}\t{v}")
    for p, i, v in rust_candidates():
        print(f"{p.relative_to(ROOT)}:{i}\t{v}")


def load_en() -> dict:
    p = LANG_DIR / "en.toml"
    if not p.exists():
        return {}
    return flatten(tomllib.loads(p.read_text(encoding="utf-8")))


def flatten(doc: dict, prefix: str = "") -> dict:
    out = {}
    for k, v in doc.items():
        key = f"{prefix}.{k}" if prefix else k
        if isinstance(v, dict):
            out.update(flatten(v, key))
        else:
            out[key] = v
    return out


def toml_str(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def write_en(table: dict):
    """Write en.toml grouped by namespace, keys sorted, dotted keys inside
    `[section]` tables (`title.practice` → `[title] practice`)."""
    groups = {}
    for key, value in table.items():
        ns, _, rest = key.partition(".")
        groups.setdefault(ns, {})[rest] = value
    out = [
        "# rugen UI strings (English, the source). One `[section]` per screen,",
        "# `common` for words several screens share. Screens refer to a key as",
        "# `text = \"@section.key\"`; Rust code with `t(\"section.key\")`.",
        "# `{name}` placeholders are filled at run time: keep them in every",
        "# translation. A key written in CAPITALS here is drawn in capitals in",
        "# every Latin-script language, whatever the translation's own case.",
        "",
    ]
    for ns in sorted(groups):
        out.append(f"[{ns}]")
        for k in sorted(groups[ns]):
            out.append(f"{quote_key(k)} = {toml_str(groups[ns][k])}")
        out.append("")
    LANG_DIR.mkdir(parents=True, exist_ok=True)
    (LANG_DIR / "en.toml").write_text("\n".join(out), encoding="utf-8")


def quote_key(k: str) -> str:
    return k if re.fullmatch(r"[A-Za-z0-9_.-]+", k) else toml_str(k)


def cmd_convert_toml():
    en = load_en()
    by_value = {}  # value -> key already in en.toml
    for k, v in en.items():
        by_value.setdefault(v, k)
    lits = list(toml_literals())
    # a value used by several screens goes to `common`
    screens_of = {}
    for p, _i, _ind, _k, v, _t in lits:
        screens_of.setdefault(v.rstrip(), set()).add(namespace(p))
    assigned = {}
    used = set(en.keys())
    for p, _i, _ind, _k, v, _t in lits:
        v = v.rstrip()
        if v in assigned:
            continue
        if v in by_value:
            assigned[v] = by_value[v]
            continue
        ns = "common" if len(screens_of[v]) > 1 else namespace(p)
        base = f"{ns}.{slug(v)}"
        key, n = base, 2
        while key in used:
            key = f"{base}_{n}"
            n += 1
        used.add(key)
        assigned[v] = key
        en[key] = v
    # rewrite the sources
    for path in TOML_SOURCES + RUST_TOML_SOURCES:
        lines = path.read_text(encoding="utf-8").split("\n")
        changed = False
        for idx, line in enumerate(lines):
            m = LINE_RE.match(line)
            if not m or not user_facing(m.group(3)):
                continue
            value = m.group(3)
            key = assigned[value.rstrip()]
            pad = value[len(value.rstrip()):]  # spacing kept outside the key
            lines[idx] = f'{m.group(1)}{m.group(2)} = "@{key}{pad}"{m.group(4)}'
            changed = True
        if changed:
            path.write_text("\n".join(lines), encoding="utf-8")
    write_en(en)
    print(f"{len(assigned)} literals -> {len(en)} keys in motif/lang/en.toml")


PLACEHOLDER_RE = re.compile(r"\{([^{}]*)\}")


def cmd_check() -> int:
    en = load_en()
    bad = 0
    files = sorted(p for p in LANG_DIR.glob("*.toml") if p.stem != "en")
    for p in files:
        table = flatten(tomllib.loads(p.read_text(encoding="utf-8")))
        missing = sorted(k for k in en if k not in table)
        extra = sorted(k for k in table if k not in en)
        empty = sorted(k for k, v in table.items() if not str(v).strip())
        mismatch = []
        same = []
        for k, v in table.items():
            if k in en and sorted(PLACEHOLDER_RE.findall(str(v))) != sorted(
                PLACEHOLDER_RE.findall(str(en[k]))
            ):
                mismatch.append(k)
            if k in en and str(v) == str(en[k]) and LETTERS_RE.search(
                BIND_RE.sub("", str(v))
            ) and len(BIND_RE.sub("", str(v)).strip()) > 3:
                same.append(k)
        name = p.stem
        for label, keys in (("missing", missing), ("extra", extra), ("empty", empty), ("placeholders", mismatch)):
            if keys:
                bad += len(keys)
                print(f"{name}: {len(keys)} {label}: {', '.join(keys[:12])}{' ...' if len(keys) > 12 else ''}")
        if same:
            print(f"{name}: {len(same)} identical to English (fine for names): {', '.join(same[:8])}{' ...' if len(same) > 8 else ''}")
        print(f"{name}: {len(table)} keys, {len(en)} in en")
    return 1 if bad else 0


def cmd_kanji():
    p = LANG_DIR / "ja.toml"
    text = "".join(str(v) for v in flatten(tomllib.loads(p.read_text(encoding="utf-8"))).values())
    chars = sorted({c for c in text if ord(c) >= 0x3000})
    print("".join(chars))
    print(len(chars), "characters", file=sys.stderr)


def main():
    os.chdir(ROOT)
    cmd = sys.argv[1] if len(sys.argv) > 1 else "list"
    if cmd == "list":
        cmd_list()
    elif cmd == "convert-toml":
        cmd_convert_toml()
    elif cmd == "check":
        sys.exit(cmd_check())
    elif cmd == "kanji":
        cmd_kanji()
    else:
        print(__doc__)
        sys.exit(2)


if __name__ == "__main__":
    main()
