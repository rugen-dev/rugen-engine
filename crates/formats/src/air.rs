//! AIR animation files.
//!
//! Semantics follow MUGEN 1.1 / Ikemen GO: duplicate action numbers keep the
//! first, an action with no frames takes the next non-empty action in the
//! file, `Copy Action n` chains resolve up to 8 deep and are dropped if they
//! cannot be resolved. Errors inside an action do not fail the file; they are
//! logged and the action keeps what parsed.

use std::collections::BTreeMap;

use crate::text::{atof, atoi, section, strip_comment};

/// Collision box `[x1, y1, x2, y2]`, normalized so `x1 <= x2`, `y1 <= y2`.
pub type Rect = [i32; 4];

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blend {
    #[default]
    None,
    Add,
    Sub,
    SubAdd,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct Frame {
    pub group: i32,
    pub number: i32,
    /// Offsets with flips already applied (H negates x, V negates y), as MUGEN does.
    pub x: i32,
    pub y: i32,
    /// Ticks; `-1` = hold forever.
    pub time: i32,
    pub hflip: bool,
    pub vflip: bool,
    pub blend: Blend,
    pub src_alpha: u8,
    pub dst_alpha: u8,
    pub xscale: f32,
    pub yscale: f32,
    /// Degrees.
    pub angle: f32,
    pub clsn1: Vec<Rect>,
    pub clsn2: Vec<Rect>,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            group: -1,
            number: 0,
            x: 0,
            y: 0,
            time: -1,
            hflip: false,
            vflip: false,
            blend: Blend::None,
            src_alpha: 255,
            dst_alpha: 0,
            xscale: 1.0,
            yscale: 1.0,
            angle: 0.0,
            clsn1: Vec::new(),
            clsn2: Vec::new(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Action {
    pub frames: Vec<Frame>,
    /// Frame index to loop back to.
    pub loop_start: usize,
    /// Frame indices that have `Interpolate X` lines before them.
    pub interpolate_offset: Vec<usize>,
    pub interpolate_scale: Vec<usize>,
    pub interpolate_angle: Vec<usize>,
    pub interpolate_blend: Vec<usize>,
    /// Sum of frame times, `-1` when the last frame holds forever.
    pub total_time: i32,
    pub loop_time: i32,
    pub pre_loop_time: i32,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct Air {
    pub actions: BTreeMap<i32, Action>,
}

impl Air {
    pub fn get(&self, no: i32) -> Option<&Action> {
        self.actions.get(&no)
    }

    pub fn parse(text: &str) -> Self {
        Parser {
            lines: text.lines().collect(),
            i: 0,
            file: "",
        }
        .parse_all()
    }

    /// Parse with a file name for log messages.
    pub fn parse_named(file: &str, text: &str) -> Self {
        Parser {
            lines: text.lines().collect(),
            i: 0,
            file,
        }
        .parse_all()
    }

    pub fn open(path: impl AsRef<std::path::Path>) -> crate::Result<Self> {
        let path = path.as_ref();
        let text = crate::text::decode(&crate::vfs::read(path)?);
        Ok(Self::parse_named(&path.to_string_lossy(), &text))
    }
}

enum Parsed {
    Frames(Action),
    Copy(i32),
}

struct Parser<'a> {
    lines: Vec<&'a str>,
    i: usize,
    file: &'a str,
}

impl Parser<'_> {
    fn parse_all(mut self) -> Air {
        // file order matters for the "empty action copies the next one" rule
        let mut order: Vec<(i32, Parsed)> = Vec::new();
        while let Some((no, parsed)) = self.next_action() {
            if order.iter().any(|(n, _)| *n == no) {
                log::warn!("{}: duplicate action {no} ignored", self.file);
                continue;
            }
            order.push((no, parsed));
        }

        // empty actions take the next non-empty one in the file
        let mut i = order.len();
        while i > 0 {
            i -= 1;
            let empty = matches!(&order[i].1, Parsed::Frames(a) if a.frames.is_empty());
            if empty {
                log::warn!("{}: action {} has no valid frames", self.file, order[i].0);
                if let Some(next) = order[i + 1..].iter().find(|(_, p)| match p {
                    Parsed::Frames(a) => !a.frames.is_empty(),
                    Parsed::Copy(_) => true,
                }) {
                    let cloned = match &next.1 {
                        Parsed::Frames(a) => Parsed::Frames(a.clone()),
                        Parsed::Copy(n) => Parsed::Copy(*n),
                    };
                    order[i].1 = cloned;
                }
            }
        }

        // resolve Copy Action chains
        let table: BTreeMap<i32, &Parsed> = order.iter().map(|(n, p)| (*n, p)).collect();
        let mut air = Air::default();
        for (no, parsed) in &order {
            let mut cur = parsed;
            let mut src_no = *no;
            let mut ok = true;
            for _ in 0..8 {
                match cur {
                    Parsed::Frames(_) => break,
                    Parsed::Copy(target) => match table.get(target) {
                        Some(p) if *target != src_no => {
                            src_no = *target;
                            cur = p;
                        }
                        _ => {
                            ok = false;
                            break;
                        }
                    },
                }
            }
            match cur {
                Parsed::Frames(a) if ok => {
                    air.actions.insert(*no, a.clone());
                }
                _ => log::warn!("{}: action {no}: unresolved Copy Action", self.file),
            }
        }
        air
    }

    /// Advance to the next `[Begin Action n]` and parse its body.
    fn next_action(&mut self) -> Option<(i32, Parsed)> {
        while self.i < self.lines.len() {
            let line = self.lines[self.i];
            self.i += 1;
            if let Some((name, rest)) = section(line)
                && name == "begin "
                && let Some(num) = rest
                    .get(..7)
                    .filter(|s| s.eq_ignore_ascii_case("action "))
                    .map(|_| &rest[7..])
            {
                let no = atoi(num);
                return Some((no, self.action_body(no)));
            }
        }
        None
    }

    fn action_body(&mut self, no: i32) -> Parsed {
        let mut a = Action::default();
        let (mut clsn1, mut clsn1d, mut clsn2, mut clsn2d): (
            Vec<Rect>,
            Vec<Rect>,
            Vec<Rect>,
            Vec<Rect>,
        ) = Default::default();
        let (mut def1, mut def2) = (true, true);
        let mut old_loop = 0usize;

        while self.i < self.lines.len() {
            let raw = self.lines[self.i];
            if raw.trim_start().starts_with('[') {
                break;
            }
            let line = strip_comment(raw).to_ascii_lowercase();
            self.i += 1;

            if let Some(rest) = line.strip_prefix("copy action ") {
                return match rest.trim().parse::<i32>() {
                    Ok(n) => Parsed::Copy(n),
                    Err(_) => Parsed::Copy(-1),
                };
            }

            match parse_frame(&line) {
                Ok(Some(mut f)) => {
                    old_loop = a.loop_start;
                    if def1 {
                        clsn1 = clsn1d.clone();
                    }
                    if def2 {
                        clsn2 = clsn2d.clone();
                    }
                    f.clsn1 = clsn1.clone();
                    f.clsn2 = clsn2.clone();
                    a.frames.push(f);
                    def1 = true;
                    def2 = true;
                    continue;
                }
                Ok(None) => {}
                Err(e) => {
                    log::warn!("{}: action {no} line {}: {e}", self.file, self.i);
                    continue;
                }
            }

            if line.starts_with("loopstart") {
                a.loop_start = a.frames.len();
            } else if line.starts_with("interpolate offset") {
                a.interpolate_offset.push(a.frames.len());
            } else if line.starts_with("interpolate scale") {
                a.interpolate_scale.push(a.frames.len());
            } else if line.starts_with("interpolate angle") {
                a.interpolate_angle.push(a.frames.len());
            } else if line.starts_with("interpolate blend") {
                a.interpolate_blend.push(a.frames.len());
            } else if line.starts_with("clsn") && line.len() >= 5 {
                let Some(colon) = line.find(':') else {
                    continue;
                };
                let size = atoi(&line[colon + 1..]);
                if size < 0 {
                    continue;
                }
                let kind = line.as_bytes()[4];
                let is_default = line[5..].starts_with("default");
                let boxes = self.clsn_boxes(no, kind, size as usize);
                match kind {
                    b'1' => {
                        clsn1 = boxes;
                        if is_default {
                            clsn1d = clsn1.clone();
                        }
                        def1 = false;
                    }
                    b'2' => {
                        clsn2 = boxes;
                        if is_default {
                            clsn2d = clsn2.clone();
                        }
                        def2 = false;
                    }
                    _ => {}
                }
            }
        }

        if a.loop_start >= a.frames.len() {
            a.loop_start = old_loop;
        }
        compute_times(&mut a);
        Parsed::Frames(a)
    }

    /// Read `size` `ClsnN[i] = x1,y1,x2,y2` lines following a `ClsnN: size` header.
    fn clsn_boxes(&mut self, no: i32, kind: u8, size: usize) -> Vec<Rect> {
        let mut boxes = vec![[0i32; 4]; size];
        if size == 0 {
            return boxes;
        }
        let mut filled = vec![false; size];
        let header_line = self.i;
        let mut count = 0;
        while count < size && self.i < self.lines.len() {
            let line = strip_comment(self.lines[self.i]).to_ascii_lowercase();
            if line.is_empty() {
                self.i += 1;
                continue;
            }
            if !line.starts_with("clsn") {
                break;
            }
            let Some(eq) = line.find('=') else { break };
            let mut idx: Option<usize> = None;
            if let Some(bs) = line.find('[')
                && bs < eq
                && let Some(be) = line[bs..].find(']')
            {
                let s = line[bs + 1..bs + be].trim();
                if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
                    idx = Some(atoi(s) as usize);
                }
            }
            let slot = match idx {
                Some(i) if i < size && !filled[i] => i,
                requested => {
                    let Some(free) = filled.iter().position(|f| !f) else {
                        self.i += 1;
                        continue;
                    };
                    if let Some(r) = requested {
                        log::warn!(
                            "{}: action {no} line {header_line}: Clsn{}[{r}] duplicate or out of range, placed in [{free}]",
                            self.file,
                            kind as char
                        );
                    }
                    free
                }
            };
            let nums: Vec<&str> = line[eq + 1..].split(',').collect();
            if nums.len() < 4 {
                break;
            }
            boxes[slot] = normalize([atoi(nums[0]), atoi(nums[1]), atoi(nums[2]), atoi(nums[3])]);
            filled[slot] = true;
            count += 1;
            self.i += 1;
        }
        if count != size {
            log::warn!(
                "{}: action {no} line {header_line}: Clsn{} declared {size} boxes, found {count}",
                self.file,
                kind as char
            );
        }
        boxes
    }
}

fn normalize(mut r: Rect) -> Rect {
    if r[0] > r[2] {
        r.swap(0, 2);
    }
    if r[1] > r[3] {
        r.swap(1, 3);
    }
    r
}

fn compute_times(a: &mut Action) {
    let Some(last) = a.frames.last() else { return };
    if last.time == -1 {
        a.total_time = -1;
        return;
    }
    let mut tmp = 0;
    for (i, f) in a.frames.iter().enumerate() {
        if f.time == -1 {
            a.total_time = 0;
            a.loop_time = -tmp;
            a.pre_loop_time = 0;
        }
        a.total_time += f.time;
        if i < a.loop_start {
            a.pre_loop_time += f.time;
            tmp += f.time;
        } else {
            a.loop_time += f.time;
        }
    }
    if a.total_time == -1 {
        a.pre_loop_time = 0;
    }
}

/// Parse one element line (already lowercased, comment stripped).
/// `Ok(None)` = not an element line.
fn parse_frame(line: &str) -> Result<Option<Frame>, String> {
    let first = line.as_bytes().first().copied();
    if !matches!(first, Some(b'0'..=b'9') | Some(b'-')) {
        return Ok(None);
    }
    let p: Vec<&str> = line.splitn(10, ',').map(str::trim).collect();
    if p.len() < 5 {
        return Err("invalid animation element".into());
    }
    let mut f = Frame {
        group: atoi(p[0]),
        number: atoi(p[1]),
        x: atoi(p[2]),
        y: atoi(p[3]),
        time: atoi(p[4]),
        ..Default::default()
    };
    let mut err: Option<String> = None;

    if let Some(flags) = p.get(5).filter(|s| !s.is_empty()) {
        for c in flags.chars() {
            match c {
                'h' => {
                    f.hflip = true;
                    f.x = -f.x;
                }
                'v' => {
                    f.vflip = true;
                    f.y = -f.y;
                }
                _ => err = Some(format!("invalid flip flag {c:?}")),
            }
        }
    }

    if let Some(t) = p.get(6).filter(|s| !s.is_empty()) {
        let t = match t.find(['a', 's']) {
            Some(i) => &t[i..],
            None => t,
        };
        let bad = |s: &str| format!("invalid transparency {s:?}");
        if t == "a1" {
            f.blend = Blend::Add;
            f.src_alpha = 255;
            f.dst_alpha = 128;
        } else if let Some(r) = t.strip_prefix("sas") {
            f.blend = Blend::SubAdd;
            (f.src_alpha, f.dst_alpha) = alpha_pair(r, 255, 255);
        } else if let Some(r) = t.strip_prefix("sa") {
            f.blend = Blend::SubAdd;
            (f.src_alpha, f.dst_alpha) = (255, 255);
            if !r.is_empty() {
                err = Some(bad(t));
            }
        } else if let Some(r) = t.strip_prefix("as") {
            f.blend = Blend::Add;
            (f.src_alpha, f.dst_alpha) = alpha_pair(r, 255, 0);
        } else if let Some(r) = t.strip_prefix("ss") {
            f.blend = Blend::Sub;
            (f.src_alpha, f.dst_alpha) = alpha_pair(r, 255, 255);
        } else if let Some(r) = t.strip_prefix('a') {
            f.blend = Blend::Add;
            (f.src_alpha, f.dst_alpha) = (255, 255);
            if !r.is_empty() {
                err = Some(bad(t));
            }
        } else if let Some(r) = t.strip_prefix('s') {
            f.blend = Blend::Sub;
            (f.src_alpha, f.dst_alpha) = (255, 255);
            if !r.is_empty() {
                err = Some(bad(t));
            }
        } else {
            err = Some(bad(t));
        }
    }

    for (i, dst, name) in [
        (7, &mut f.xscale, "x-scale"),
        (8, &mut f.yscale, "y-scale"),
        (9, &mut f.angle, "angle"),
    ] {
        if let Some(s) = p.get(i).filter(|s| !s.is_empty()) {
            match atof(s) {
                Some(v) => *dst = v,
                None => err = Some(format!("invalid {name} {s:?}")),
            }
        }
    }

    match err {
        Some(e) => Err(e),
        None => Ok(Some(f)),
    }
}

/// `"64d192"` → (64, 192); missing parts take defaults; values clamp to 255.
fn alpha_pair(s: &str, def_src: u8, def_dst: u8) -> (u8, u8) {
    let digits = |s: &str| -> (Option<u32>, usize) {
        let n = s.bytes().take_while(u8::is_ascii_digit).count();
        (
            if n == 0 {
                None
            } else {
                s[..n].parse::<u32>().ok()
            },
            n,
        )
    };
    let (src, n) = digits(s);
    let src = src.map_or(def_src, |v| (v & 0x3FFF).min(255) as u8);
    let mut dst = def_dst;
    if let Some(r) = s[n..].strip_prefix('d') {
        if let (Some(v), _) = digits(r) {
            dst = (v & 0x3FFF).min(255) as u8;
        }
    }
    (src, dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::reference;

    #[test]
    fn element_basics() {
        let f = parse_frame("0,3, 0,0, 7").unwrap().unwrap();
        assert_eq!((f.group, f.number, f.x, f.y, f.time), (0, 3, 0, 0, 7));
        assert_eq!(f.blend, Blend::None);
        let f = parse_frame("15,1, 10,-5, 5, h").unwrap().unwrap();
        assert!(f.hflip && !f.vflip);
        assert_eq!((f.x, f.y), (-10, -5));
        let f = parse_frame("15,4, 0,0, 5, ,as64d192, 1.5,2, 45")
            .unwrap()
            .unwrap();
        assert_eq!(f.blend, Blend::Add);
        assert_eq!((f.src_alpha, f.dst_alpha), (64, 192));
        assert_eq!((f.xscale, f.yscale, f.angle), (1.5, 2.0, 45.0));
        let f = parse_frame("1,1,0,0,1,,a1").unwrap().unwrap();
        assert_eq!((f.blend, f.src_alpha, f.dst_alpha), (Blend::Add, 255, 128));
        let f = parse_frame("1,1,0,0,1,,s").unwrap().unwrap();
        assert_eq!(f.blend, Blend::Sub);
        assert!(parse_frame("clsn2: 1").unwrap().is_none());
        assert!(parse_frame("1,2,3").is_err());
    }

    #[test]
    fn action_with_clsn_and_loop() {
        let air = Air::parse(
            "[Begin Action 0]\nClsn2Default: 2\n Clsn2[0] = -10, 0, 10,-79\n Clsn2[1] = 6,-79, -4,-92\n0,1, 0,0, 7\nLoopstart\nClsn1: 1\n Clsn1[0] = 0,0,5,5\n0,2, 0,0, 3\n0,3, 0,0, -1\n",
        );
        let a = air.get(0).unwrap();
        assert_eq!(a.frames.len(), 3);
        assert_eq!(a.loop_start, 1);
        assert_eq!(
            a.frames[0].clsn2,
            vec![[-10, -79, 10, 0], [-4, -92, 6, -79]]
        );
        assert_eq!(a.frames[1].clsn1, vec![[0, 0, 5, 5]]);
        assert!(
            a.frames[2].clsn1.is_empty(),
            "clsn1 is not default, applies to one frame"
        );
        assert_eq!(a.frames[2].clsn2.len(), 2, "clsn2 default persists");
        assert_eq!(a.total_time, -1);
    }

    #[test]
    fn times() {
        let a = Air::parse("[Begin Action 1]\n0,0,0,0,7\n0,0,0,0,7\nLoopstart\n0,0,0,0,50\n");
        let a = a.get(1).unwrap();
        assert_eq!((a.total_time, a.pre_loop_time, a.loop_time), (64, 14, 50));
    }

    #[test]
    fn copy_and_empty_and_duplicate() {
        let air = Air::parse(
            "[Begin Action 5]\n[Begin Action 6]\n0,0,0,0,1\n[Begin Action 6]\n0,0,0,0,99\n[Begin Action 7]\nCopy Action 6\n[Begin Action 8]\nCopy Action 999\n[Begin Action 9]\nCopy Action 9\n",
        );
        assert_eq!(
            air.get(5).unwrap().frames[0].time,
            1,
            "empty action takes next non-empty"
        );
        assert_eq!(
            air.get(6).unwrap().frames[0].time,
            1,
            "first duplicate wins"
        );
        assert_eq!(air.get(7).unwrap().frames[0].time, 1, "copy action");
        assert!(air.get(8).is_none(), "unresolved copy dropped");
        assert!(air.get(9).is_none(), "self copy dropped");
    }

    #[test]
    fn kfm_air() {
        let Some(p) = reference("mugen-1.1b1/chars/kfm/kfm.air") else {
            return;
        };
        let air = Air::open(p).unwrap();
        assert_eq!(air.actions.len(), 117);
        let stand = air.get(0).unwrap();
        assert_eq!(stand.frames.len(), 11);
        assert_eq!(stand.frames[0].clsn2.len(), 2);
        assert_eq!(stand.total_time, 151);
        let turn = air.get(5).unwrap();
        assert!(turn.frames[0].hflip);
        // 200 = standing light punch: has clsn1 on the active frame
        let lp = air.get(200).unwrap();
        assert!(lp.frames.iter().any(|f| !f.clsn1.is_empty()));
    }

    #[test]
    fn all_reference_airs_parse() {
        let Some(root) = reference("") else { return };
        let mut n = 0;
        for dir in ["mugen-1.1b1", "ikemen-go-v1.0.0"] {
            for p in crate::fixtures::walk(&root.join(dir)) {
                if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("air")) {
                    let air = Air::open(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                    assert!(!air.actions.is_empty(), "{} has no actions", p.display());
                    n += 1;
                }
            }
        }
        if n == 0 {
            eprintln!("reference tree missing; skipping");
            return;
        }
        assert!(n >= 5, "found only {n} air files");
    }
}
