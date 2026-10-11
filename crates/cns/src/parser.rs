//! Expression compiler: tokens → [`Code`]. Precedence (low → high):
//! `||`  `^^`  `&&`  `|`  `^`  `&`  `:=`  `= != intervals`  `> >= < <=`
//! `+ -`  `* / %`  `**`  unary. `:=` only follows `var(n)` and its right side
//! is parsed at the `=` level, as the MUGEN table implies.

use crate::code::{Cmp, Code, Op, Redirect, VarKind};
use crate::lexer::{Tok, lex};
use crate::trigger::{Axis, NameWhich, ProjEvent, TeamMode, Trigger, flags};
use crate::{Error, Result};

/// Compile one expression; all input must be consumed.
pub fn compile(src: &str) -> Result<Code> {
    let toks = lex(src)?;
    let mut p = Parser {
        toks: &toks,
        pos: 0,
        out: Vec::new(),
    };
    p.expr()?;
    if !matches!(p.peek(), Tok::End) {
        return Err(Error::new(format!(
            "unexpected {:?} after expression",
            p.peek()
        )));
    }
    Ok(Code { ops: p.out })
}

/// Compile a comma-separated list of expressions (state controller parameters).
pub fn compile_list(src: &str) -> Result<Vec<Code>> {
    let toks = lex(src)?;
    let mut p = Parser {
        toks: &toks,
        pos: 0,
        out: Vec::new(),
    };
    let mut out = Vec::new();
    if matches!(p.peek(), Tok::End) {
        return Ok(out);
    }
    loop {
        p.expr()?;
        out.push(Code {
            ops: std::mem::take(&mut p.out),
        });
        match p.peek() {
            Tok::Comma => {
                p.pos += 1;
                if matches!(p.peek(), Tok::End) {
                    break; // trailing comma
                }
            }
            Tok::End => break,
            t => return Err(Error::new(format!("unexpected {t:?} in parameter list"))),
        }
    }
    Ok(out)
}

/// Names the expression parser understands (triggers, functions, redirects).
/// Used to decide whether an `S`/`F` prefix on a parameter is an FX prefix
/// (`S1,0`) or the start of a trigger (`sysvar(0)`, `facing`).
pub fn is_known_name(name: &str) -> bool {
    const NAMES: &[&str] = &[
        "var",
        "fvar",
        "sysvar",
        "sysfvar",
        "ifelse",
        "cond",
        "abs",
        "floor",
        "ceil",
        "exp",
        "ln",
        "sin",
        "cos",
        "tan",
        "asin",
        "acos",
        "atan",
        "log",
        "pi",
        "e",
        "const",
        "gethitvar",
        "const240p",
        "const480p",
        "const720p",
        "animelemtime",
        "animelemno",
        "animexist",
        "selfanimexist",
        "numprojid",
        "playeridexist",
        "projhittime",
        "projguardedtime",
        "projcontacttime",
        "projcanceltime",
        "numhelper",
        "numexplod",
        "numtarget",
        "ishelper",
        "pos",
        "vel",
        "hitvel",
        "camerapos",
        "screenpos",
        "p2dist",
        "p2bodydist",
        "rootdist",
        "parentdist",
        "command",
        "name",
        "p1name",
        "p2name",
        "p3name",
        "p4name",
        "authorname",
        "stagevar",
        "statetype",
        "p2statetype",
        "movetype",
        "p2movetype",
        "teammode",
        "hitdefattr",
        "animelem",
        "timemod",
        "parent",
        "root",
        "helper",
        "target",
        "partner",
        "enemy",
        "enemynear",
        "playerid",
        "p2",
        "ailevel",
        "alive",
        "anim",
        "animtime",
        "backedge",
        "backedgebodydist",
        "backedgedist",
        "bottomedge",
        "camerazoom",
        "canrecover",
        "ctrl",
        "drawgame",
        "facing",
        "frontedge",
        "frontedgebodydist",
        "frontedgedist",
        "gameheight",
        "gametime",
        "gamewidth",
        "hitcount",
        "hitfall",
        "hitover",
        "hitpausetime",
        "hitshakeover",
        "id",
        "inguarddist",
        "ishometeam",
        "leftedge",
        "life",
        "lifemax",
        "lose",
        "matchno",
        "matchover",
        "movecontact",
        "moveguarded",
        "movehit",
        "movereversed",
        "numenemy",
        "numpartner",
        "numproj",
        "p2life",
        "p2stateno",
        "palno",
        "power",
        "powermax",
        "prevstateno",
        "random",
        "rightedge",
        "roundno",
        "roundsexisted",
        "roundstate",
        "screenheight",
        "screenwidth",
        "stateno",
        "teamside",
        "tickspersecond",
        "time",
        "statetime",
        "topedge",
        "uniqhitcount",
        "win",
    ];
    NAMES.contains(&name)
        || name.starts_with("projhit")
        || name.starts_with("projguarded")
        || name.starts_with("projcontact")
}

/// Split an FX prefix (`S`/`F`, case-insensitive) off a parameter value, as
/// MUGEN does for sound and animation parameters: `S1,0` → (Some('s'), "1,0"),
/// `sysvar(0)` → (None, "sysvar(0)"), `F30` → (Some('f'), "30").
pub fn strip_fx_prefix(value: &str) -> (Option<char>, &str) {
    let v = value.trim();
    let Some(first) = v.chars().next() else {
        return (None, v);
    };
    let p = first.to_ascii_lowercase();
    if p != 's' && p != 'f' || v.len() < 2 {
        return (None, v);
    }
    let rest = &v[1..];
    let word: String = rest
        .trim_start_matches(|c: char| !c.is_ascii_alphabetic())
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let word = word.to_ascii_lowercase();
    // "S1,0": nothing alphabetic follows → prefix. "sysvar(0)": "ysvar" is not a name → no prefix.
    // "Stime" would be prefix + trigger; "s" alone is not.
    if rest
        .trim_start()
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        if is_known_name(&word) {
            (Some(p), rest)
        } else {
            (None, v)
        }
    } else {
        (Some(p), rest)
    }
}

struct Parser<'a> {
    toks: &'a [Tok],
    pos: usize,
    out: Vec<Op>,
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }
    fn peek2(&self) -> &Tok {
        &self.toks[(self.pos + 1).min(self.toks.len() - 1)]
    }
    fn next(&mut self) -> Tok {
        let t = self.peek().clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn eat_op(&mut self, s: &str) -> bool {
        if self.peek().is_op(s) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: Tok, what: &str) -> Result<()> {
        if *self.peek() == t {
            self.pos += 1;
            Ok(())
        } else {
            Err(Error::new(format!(
                "expected {what}, found {:?}",
                self.peek()
            )))
        }
    }
    fn emit(&mut self, op: Op) {
        self.out.push(op);
    }
    fn patch_jump(&mut self, at: usize) {
        let off = (self.out.len() - at - 1) as u32;
        match &mut self.out[at] {
            Op::Jz(n) | Op::Jnz(n) | Op::Jmp(n) | Op::Jbottom(n) => *n = off,
            Op::Redirect { len, .. } => *len = off,
            _ => unreachable!(),
        }
    }

    // ---------------------------------------------------------- precedence

    fn expr(&mut self) -> Result<()> {
        self.lor()
    }

    fn lor(&mut self) -> Result<()> {
        self.lxor()?;
        while self.eat_op("||") {
            self.emit(Op::ToBool);
            let j = self.out.len();
            self.emit(Op::Jnz(0));
            self.emit(Op::Pop);
            self.lxor()?;
            self.emit(Op::ToBool);
            self.patch_jump(j);
        }
        Ok(())
    }

    fn lxor(&mut self) -> Result<()> {
        self.land()?;
        while self.eat_op("^^") {
            self.land()?;
            self.emit(Op::LXor);
        }
        Ok(())
    }

    fn land(&mut self) -> Result<()> {
        self.bor()?;
        while self.eat_op("&&") {
            self.emit(Op::ToBool);
            let j = self.out.len();
            self.emit(Op::Jz(0));
            self.emit(Op::Pop);
            self.bor()?;
            self.emit(Op::ToBool);
            self.patch_jump(j);
        }
        Ok(())
    }

    fn bor(&mut self) -> Result<()> {
        self.bxor()?;
        while self.eat_op("|") {
            self.bxor()?;
            self.emit(Op::BitOr);
        }
        Ok(())
    }
    fn bxor(&mut self) -> Result<()> {
        self.band()?;
        while self.eat_op("^") {
            self.band()?;
            self.emit(Op::BitXor);
        }
        Ok(())
    }
    fn band(&mut self) -> Result<()> {
        self.eqne()?;
        while self.eat_op("&") {
            self.eqne()?;
            self.emit(Op::BitAnd);
        }
        Ok(())
    }

    fn eqne(&mut self) -> Result<()> {
        self.cmp()?;
        loop {
            let ne = if self.eat_op("=") {
                false
            } else if self.eat_op("!=") {
                true
            } else {
                return Ok(());
            };
            if self.interval()? {
                if ne {
                    self.emit(Op::LNot);
                }
                continue;
            }
            self.cmp()?;
            self.emit(if ne { Op::Ne } else { Op::Eq });
        }
    }

    /// After `=`/`!=`: `[lo, hi]`, `(lo, hi)`, or mixed. Returns false (nothing
    /// consumed) when a `(` turns out to be a parenthesised expression.
    fn interval(&mut self) -> Result<bool> {
        let lo_incl = match self.peek() {
            Tok::LBracket => true,
            Tok::LParen => false,
            _ => return Ok(false),
        };
        let save = (self.pos, self.out.len());
        self.pos += 1;
        self.expr()?;
        if !matches!(self.peek(), Tok::Comma) {
            if lo_incl {
                return Err(Error::new("interval missing ','"));
            }
            self.pos = save.0;
            self.out.truncate(save.1);
            return Ok(false);
        }
        self.pos += 1;
        self.expr()?;
        let hi_incl = match self.next() {
            Tok::RBracket => true,
            Tok::RParen => false,
            t => {
                return Err(Error::new(format!(
                    "interval missing ']' or ')', found {t:?}"
                )));
            }
        };
        self.emit(Op::Range { lo_incl, hi_incl });
        Ok(true)
    }

    fn cmp(&mut self) -> Result<()> {
        self.add()?;
        loop {
            let op = if self.eat_op("<") {
                Op::Lt
            } else if self.eat_op("<=") {
                Op::Le
            } else if self.eat_op(">") {
                Op::Gt
            } else if self.eat_op(">=") {
                Op::Ge
            } else {
                return Ok(());
            };
            self.add()?;
            self.emit(op);
        }
    }

    fn add(&mut self) -> Result<()> {
        self.mul()?;
        loop {
            let op = if self.eat_op("+") {
                Op::Add
            } else if self.eat_op("-") {
                Op::Sub
            } else {
                return Ok(());
            };
            self.mul()?;
            self.emit(op);
        }
    }

    fn mul(&mut self) -> Result<()> {
        self.pow()?;
        loop {
            let op = if self.eat_op("*") {
                Op::Mul
            } else if self.eat_op("/") {
                Op::Div
            } else if self.eat_op("%") {
                Op::Mod
            } else {
                return Ok(());
            };
            self.pow()?;
            self.emit(op);
        }
    }

    fn pow(&mut self) -> Result<()> {
        self.unary()?;
        while self.eat_op("**") {
            self.unary()?;
            self.emit(Op::Pow);
        }
        Ok(())
    }

    fn unary(&mut self) -> Result<()> {
        if self.eat_op("-") {
            // fold negative literals so `-1` is a constant, not an op
            if let Tok::Num(n) = self.peek().clone() {
                self.pos += 1;
                self.number(&format!("-{n}"))?;
                return Ok(());
            }
            self.unary()?;
            self.emit(Op::Neg);
        } else if self.eat_op("!") {
            self.unary()?;
            self.emit(Op::LNot);
        } else if self.eat_op("~") {
            self.unary()?;
            self.emit(Op::BitNot);
        } else if self.eat_op("+") {
            self.unary()?;
        } else {
            self.primary()?;
        }
        Ok(())
    }

    fn number(&mut self, s: &str) -> Result<()> {
        if s.contains('.') || s.contains(['e', 'E']) {
            let f: f32 = s
                .parse()
                .map_err(|_| Error::new(format!("bad number {s:?}")))?;
            self.emit(Op::Float(f));
        } else {
            let i: i64 = s
                .parse()
                .map_err(|_| Error::new(format!("bad number {s:?}")))?;
            self.emit(Op::Int(i.clamp(i32::MIN as i64, i32::MAX as i64) as i32));
        }
        Ok(())
    }

    // ---------------------------------------------------------- primaries

    fn primary(&mut self) -> Result<()> {
        match self.next() {
            Tok::Num(n) => self.number(&n),
            Tok::LParen => {
                self.expr()?;
                self.expect(Tok::RParen, "')'")
            }
            Tok::Ident(name) => self.ident(&name),
            Tok::Str(s) => Err(Error::new(format!("unexpected string {s:?}"))),
            t => Err(Error::new(format!("unexpected {t:?}"))),
        }
    }

    fn paren_expr(&mut self) -> Result<()> {
        self.expect(Tok::LParen, "'('")?;
        self.expr()?;
        self.expect(Tok::RParen, "')'")
    }

    /// `(expr)` if present, else push `default`.
    fn opt_arg(&mut self, default: i32) -> Result<()> {
        if matches!(self.peek(), Tok::LParen) {
            self.paren_expr()
        } else {
            self.emit(Op::Int(default));
            Ok(())
        }
    }

    fn paren_ident(&mut self) -> Result<String> {
        self.expect(Tok::LParen, "'('")?;
        let name = match self.next() {
            Tok::Ident(s) => s,
            t => return Err(Error::new(format!("expected name, found {t:?}"))),
        };
        self.expect(Tok::RParen, "')'")?;
        Ok(name)
    }

    fn axis(&mut self) -> Result<Axis> {
        match self.next() {
            Tok::Ident(s) if s == "x" => Ok(Axis::X),
            Tok::Ident(s) if s == "y" => Ok(Axis::Y),
            t => Err(Error::new(format!("expected X or Y, found {t:?}"))),
        }
    }

    /// `=` or `!=`; returns true for `!=`.
    fn eq_or_ne(&mut self, what: &str) -> Result<bool> {
        if self.eat_op("=") {
            Ok(false)
        } else if self.eat_op("!=") {
            Ok(true)
        } else {
            Err(Error::new(format!("{what} needs '=' or '!='")))
        }
    }

    fn string(&mut self) -> Result<String> {
        match self.next() {
            Tok::Str(s) => Ok(s),
            t => Err(Error::new(format!("expected a quoted string, found {t:?}"))),
        }
    }

    fn integer(&mut self) -> Result<i32> {
        let neg = self.eat_op("-");
        match self.next() {
            Tok::Num(n) if !n.contains('.') => {
                let v: i32 = n
                    .parse()
                    .map_err(|_| Error::new(format!("bad integer {n:?}")))?;
                Ok(if neg { -v } else { v })
            }
            t => Err(Error::new(format!("expected an integer, found {t:?}"))),
        }
    }

    fn cmp_op(&mut self) -> Option<Cmp> {
        for (s, c) in [
            ("=", Cmp::Eq),
            ("!=", Cmp::Ne),
            ("<=", Cmp::Le),
            (">=", Cmp::Ge),
            ("<", Cmp::Lt),
            (">", Cmp::Gt),
        ] {
            if self.eat_op(s) {
                return Some(c);
            }
        }
        None
    }

    fn redirect(&mut self, kind: Redirect, needs_id: bool, allows_id: bool) -> Result<()> {
        let with_id = if allows_id && matches!(self.peek(), Tok::LParen) {
            self.paren_expr()?;
            true
        } else if needs_id {
            return Err(Error::new("redirect needs an id"));
        } else {
            false
        };
        self.expect(Tok::Comma, "',' after redirect")?;
        let at = self.out.len();
        self.emit(Op::Redirect {
            kind,
            with_id,
            len: 0,
        });
        self.unary()?;
        self.patch_jump(at);
        Ok(())
    }

    fn state_type_mask(s: &str) -> Option<u32> {
        let mut m = 0;
        for c in s.chars() {
            m |= match c {
                's' => flags::ST_S,
                'c' => flags::ST_C,
                'a' => flags::ST_A,
                'l' => flags::ST_L,
                'u' => flags::ST_U,
                _ => return None,
            };
        }
        Some(m)
    }

    fn move_type_mask(s: &str) -> Option<u32> {
        let mut m = 0;
        for c in s.chars() {
            m |= match c {
                'i' => flags::MT_I,
                'a' => flags::MT_A,
                'h' => flags::MT_H,
                'u' => flags::MT_U,
                _ => return None,
            };
        }
        Some(m)
    }

    fn ident(&mut self, name: &str) -> Result<()> {
        use Trigger as T;
        let nullary = |p: &mut Self, t: Trigger| {
            p.emit(Op::Trigger(t));
            Ok(())
        };
        match name {
            // variables
            "var" | "fvar" | "sysvar" | "sysfvar" => {
                let kind = match name {
                    "var" => VarKind::Var,
                    "fvar" => VarKind::FVar,
                    "sysvar" => VarKind::SysVar,
                    _ => VarKind::SysFVar,
                };
                self.paren_expr()?;
                if self.eat_op(":=") {
                    self.eqne()?;
                    self.emit(Op::SetVar(kind));
                } else {
                    self.emit(Op::GetVar(kind));
                }
                Ok(())
            }
            // conditionals
            "ifelse" => {
                self.expect(Tok::LParen, "'('")?;
                self.expr()?;
                self.expect(Tok::Comma, "','")?;
                self.expr()?;
                self.expect(Tok::Comma, "','")?;
                self.expr()?;
                self.expect(Tok::RParen, "')'")?;
                self.emit(Op::IfElse);
                Ok(())
            }
            "cond" => {
                self.expect(Tok::LParen, "'('")?;
                self.expr()?;
                self.expect(Tok::Comma, "','")?;
                let jb = self.out.len();
                self.emit(Op::Jbottom(0));
                let jz = self.out.len();
                self.emit(Op::Jz(0));
                self.emit(Op::Pop);
                self.expr()?;
                self.expect(Tok::Comma, "','")?;
                let jmp = self.out.len();
                self.emit(Op::Jmp(0));
                self.patch_jump(jz);
                self.emit(Op::Pop);
                self.expr()?;
                self.expect(Tok::RParen, "')'")?;
                self.patch_jump(jmp);
                self.patch_jump(jb);
                Ok(())
            }
            // math
            "abs" | "floor" | "ceil" | "exp" | "ln" | "sin" | "cos" | "tan" | "asin" | "acos"
            | "atan" => {
                self.paren_expr()?;
                self.emit(match name {
                    "abs" => Op::Abs,
                    "floor" => Op::Floor,
                    "ceil" => Op::Ceil,
                    "exp" => Op::Exp,
                    "ln" => Op::Ln,
                    "sin" => Op::Sin,
                    "cos" => Op::Cos,
                    "tan" => Op::Tan,
                    "asin" => Op::Asin,
                    "acos" => Op::Acos,
                    _ => Op::Atan,
                });
                Ok(())
            }
            "log" => {
                self.expect(Tok::LParen, "'('")?;
                self.expr()?;
                self.expect(Tok::Comma, "','")?;
                self.expr()?;
                self.expect(Tok::RParen, "')'")?;
                self.emit(Op::Log);
                Ok(())
            }
            "pi" => {
                self.emit(Op::Float(std::f32::consts::PI));
                Ok(())
            }
            "e" => {
                self.emit(Op::Float(std::f32::consts::E));
                Ok(())
            }
            // named constants
            "const" => {
                let n = self.paren_ident()?;
                nullary(self, T::Const(n))
            }
            "gethitvar" => {
                let n = self.paren_ident()?;
                nullary(self, T::GetHitVar(n))
            }
            "const240p" | "const480p" | "const720p" => {
                self.paren_expr()?;
                let h = match name {
                    "const240p" => 240,
                    "const480p" => 480,
                    _ => 720,
                };
                nullary(self, T::ConstCoord(h))
            }
            // one required expression argument
            "animelemtime" | "animelemno" | "animexist" | "selfanimexist" | "numprojid"
            | "playeridexist" | "projhittime" | "projguardedtime" | "projcontacttime"
            | "projcanceltime" => {
                self.paren_expr()?;
                nullary(
                    self,
                    match name {
                        "animelemtime" => T::AnimElemTime,
                        "animelemno" => T::AnimElemNo,
                        "animexist" => T::AnimExist,
                        "selfanimexist" => T::SelfAnimExist,
                        "numprojid" => T::NumProjId,
                        "playeridexist" => T::PlayerIdExist,
                        "projhittime" => T::ProjHitTime,
                        "projguardedtime" => T::ProjGuardedTime,
                        "projcontacttime" => T::ProjContactTime,
                        _ => T::ProjCancelTime,
                    },
                )
            }
            "numhelper" | "numexplod" | "numtarget" => {
                self.opt_arg(-1)?;
                nullary(
                    self,
                    match name {
                        "numhelper" => T::NumHelper,
                        "numexplod" => T::NumExplod,
                        _ => T::NumTarget,
                    },
                )
            }
            "ishelper" => {
                self.opt_arg(i32::MIN)?;
                nullary(self, T::IsHelper)
            }
            // axis triggers
            "pos" => {
                let a = self.axis()?;
                nullary(self, T::Pos(a))
            }
            "vel" => {
                let a = self.axis()?;
                nullary(self, T::Vel(a))
            }
            "hitvel" => {
                let a = self.axis()?;
                nullary(self, T::HitVel(a))
            }
            "camerapos" => {
                let a = self.axis()?;
                nullary(self, T::CameraPos(a))
            }
            "screenpos" => {
                let a = self.axis()?;
                nullary(self, T::ScreenPos(a))
            }
            "p2dist" => {
                let a = self.axis()?;
                nullary(self, T::P2Dist(a))
            }
            "p2bodydist" => {
                let a = self.axis()?;
                nullary(self, T::P2BodyDist(a))
            }
            "rootdist" => {
                let a = self.axis()?;
                nullary(self, T::RootDist(a))
            }
            "parentdist" => {
                let a = self.axis()?;
                nullary(self, T::ParentDist(a))
            }
            // string comparisons
            "command" => {
                let ne = self.eq_or_ne("Command")?;
                let s = self.string()?;
                self.emit(Op::Trigger(T::Command(s)));
                if ne {
                    self.emit(Op::LNot);
                }
                Ok(())
            }
            "name" | "p1name" | "p2name" | "p3name" | "p4name" | "authorname" => {
                let which = match name {
                    "name" => NameWhich::Name,
                    "p1name" => NameWhich::P1Name,
                    "p2name" => NameWhich::P2Name,
                    "p3name" => NameWhich::P3Name,
                    "p4name" => NameWhich::P4Name,
                    _ => NameWhich::AuthorName,
                };
                let ne = self.eq_or_ne(name)?;
                let s = self.string()?;
                self.emit(Op::Trigger(T::NameIs { which, text: s }));
                if ne {
                    self.emit(Op::LNot);
                }
                Ok(())
            }
            "stagevar" => {
                let n = self.paren_ident()?;
                let ne = self.eq_or_ne("StageVar")?;
                let s = self.string()?;
                self.emit(Op::Trigger(T::StageVarIs { name: n, text: s }));
                if ne {
                    self.emit(Op::LNot);
                }
                Ok(())
            }
            // flag comparisons
            "statetype" | "p2statetype" | "movetype" | "p2movetype" => {
                let p2 = name.starts_with("p2");
                let is_state = name.ends_with("statetype");
                if !(self.peek().is_op("=") || self.peek().is_op("!=")) {
                    return nullary(
                        self,
                        match (is_state, p2) {
                            (true, false) => T::StateType,
                            (true, true) => T::P2StateType,
                            (false, false) => T::MoveType,
                            (false, true) => T::P2MoveType,
                        },
                    );
                }
                let ne = self.eq_or_ne(name)?;
                let flag = match self.next() {
                    Tok::Ident(s) => s,
                    t => {
                        return Err(Error::new(format!(
                            "{name}: expected a type letter, found {t:?}"
                        )));
                    }
                };
                let mask = if is_state {
                    Self::state_type_mask(&flag)
                } else {
                    Self::move_type_mask(&flag)
                }
                .ok_or_else(|| Error::new(format!("{name}: invalid type {flag:?}")))?;
                self.emit(Op::Trigger(if is_state {
                    T::StateTypeIs { p2, mask }
                } else {
                    T::MoveTypeIs { p2, mask }
                }));
                if ne {
                    self.emit(Op::LNot);
                }
                Ok(())
            }
            "teammode" => {
                if !(self.peek().is_op("=") || self.peek().is_op("!=")) {
                    return nullary(self, T::TeamModeRaw);
                }
                let ne = self.eq_or_ne("TeamMode")?;
                let mode = match self.next() {
                    Tok::Ident(s) if s == "single" => TeamMode::Single,
                    Tok::Ident(s) if s == "simul" => TeamMode::Simul,
                    Tok::Ident(s) if s == "turns" => TeamMode::Turns,
                    t => return Err(Error::new(format!("TeamMode: invalid mode {t:?}"))),
                };
                self.emit(Op::Trigger(T::TeamModeIs(mode)));
                if ne {
                    self.emit(Op::LNot);
                }
                Ok(())
            }
            "hitdefattr" => {
                let ne = self.eq_or_ne("HitDefAttr")?;
                let st = match self.next() {
                    Tok::Ident(s) => Self::state_type_mask(&s)
                        .ok_or_else(|| Error::new(format!("HitDefAttr: bad state flags {s:?}")))?,
                    t => {
                        return Err(Error::new(format!(
                            "HitDefAttr: expected SCA flags, found {t:?}"
                        )));
                    }
                };
                let mut at = 0u32;
                while matches!(self.peek(), Tok::Comma) {
                    let Tok::Ident(s) = self.peek2().clone() else {
                        break;
                    };
                    let bits = match s.as_str() {
                        "na" => flags::AT_NA,
                        "nt" => flags::AT_NT,
                        "np" => flags::AT_NP,
                        "sa" => flags::AT_SA,
                        "st" => flags::AT_ST,
                        "sp" => flags::AT_SP,
                        "ha" => flags::AT_HA,
                        "ht" => flags::AT_HT,
                        "hp" => flags::AT_HP,
                        "aa" => flags::AT_HA,
                        "at" => flags::AT_HT,
                        "ap" => flags::AT_HP,
                        "n" => flags::AT_NA | flags::AT_NT | flags::AT_NP,
                        "s" => flags::AT_SA | flags::AT_ST | flags::AT_SP,
                        "h" | "a" => flags::AT_HA | flags::AT_HT | flags::AT_HP,
                        _ => break,
                    };
                    self.pos += 2;
                    at |= bits;
                }
                self.emit(Op::Trigger(T::HitDefAttrIs { st, at }));
                if ne {
                    self.emit(Op::LNot);
                }
                Ok(())
            }
            // old-style clauses
            "animelem" => {
                if !self.eat_op("=") {
                    return Err(Error::new("AnimElem needs '='"));
                }
                let n = self.integer()?;
                if n <= 0 {
                    return Err(Error::new("AnimElem must be greater than 0"));
                }
                self.emit(Op::Int(n));
                self.emit(Op::Trigger(T::AnimElemTime));
                let jb = self.out.len();
                self.emit(Op::Jbottom(0));
                self.old_comparison(false)?;
                self.patch_jump(jb);
                Ok(())
            }
            "timemod" => {
                if !self.eat_op("=") {
                    return Err(Error::new("TimeMod needs '='"));
                }
                let n = self.integer()?;
                if n <= 0 {
                    return Err(Error::new("TimeMod divisor must be greater than 0"));
                }
                self.emit(Op::Trigger(T::Time));
                self.emit(Op::Int(n));
                self.emit(Op::Mod);
                self.old_comparison(true)?;
                Ok(())
            }
            _ if name.starts_with("projhit")
                || name.starts_with("projguarded")
                || name.starts_with("projcontact") =>
            {
                let (ev, base) = if name.starts_with("projhit") {
                    (ProjEvent::Hit, "projhit")
                } else if name.starts_with("projguarded") {
                    (ProjEvent::Guarded, "projguarded")
                } else {
                    (ProjEvent::Contact, "projcontact")
                };
                let suffix = &name[base.len()..];
                let id: i32 = if suffix.is_empty() {
                    0
                } else {
                    suffix.parse().unwrap_or(0)
                };
                if !self.eat_op("=") {
                    return Err(Error::new(format!("{base} needs '='")));
                }
                let expect = self.integer()? != 0;
                let (cmp, value) =
                    if matches!(self.peek(), Tok::Comma) && !matches!(self.peek2(), Tok::End) {
                        self.pos += 1;
                        let c = self.cmp_op().unwrap_or(Cmp::Eq);
                        (c, self.integer()?)
                    } else {
                        (Cmp::Eq, 0)
                    };
                self.emit(Op::Int(id));
                self.emit(Op::Trigger(match ev {
                    ProjEvent::Hit => T::ProjHitTime,
                    ProjEvent::Guarded => T::ProjGuardedTime,
                    ProjEvent::Contact => T::ProjContactTime,
                }));
                self.emit(Op::ProjClause { cmp, value, expect });
                Ok(())
            }
            // redirection
            "parent" => self.redirect(Redirect::Parent, false, false),
            "root" => self.redirect(Redirect::Root, false, false),
            "helper" => self.redirect(Redirect::Helper, false, true),
            "target" => self.redirect(Redirect::Target, false, true),
            "partner" => self.redirect(Redirect::Partner, false, false),
            "enemy" => self.redirect(Redirect::Enemy, false, true),
            "enemynear" => self.redirect(Redirect::EnemyNear, false, true),
            "playerid" => self.redirect(Redirect::PlayerId, true, true),
            "p2" => self.redirect(Redirect::P2, false, false),
            // nullary
            _ => {
                let t = match name {
                    "ailevel" => T::AILevel,
                    "alive" => T::Alive,
                    "anim" => T::Anim,
                    "animtime" => T::AnimTime,
                    "backedge" => T::BackEdge,
                    "backedgebodydist" => T::BackEdgeBodyDist,
                    "backedgedist" => T::BackEdgeDist,
                    "bottomedge" => T::BottomEdge,
                    "camerazoom" => T::CameraZoom,
                    "canrecover" => T::CanRecover,
                    "ctrl" => T::Ctrl,
                    "drawgame" => T::DrawGame,
                    "facing" => T::Facing,
                    "frontedge" => T::FrontEdge,
                    "frontedgebodydist" => T::FrontEdgeBodyDist,
                    "frontedgedist" => T::FrontEdgeDist,
                    "gameheight" => T::GameHeight,
                    "gametime" => T::GameTime,
                    "gamewidth" => T::GameWidth,
                    "hitcount" => T::HitCount,
                    "hitfall" => T::HitFall,
                    "hitover" => T::HitOver,
                    "hitpausetime" => T::HitPauseTime,
                    "hitshakeover" => T::HitShakeOver,
                    "id" => T::Id,
                    "inguarddist" => T::InGuardDist,
                    "ishometeam" => T::IsHomeTeam,
                    "leftedge" => T::LeftEdge,
                    "life" => T::Life,
                    "lifemax" => T::LifeMax,
                    "lose" => T::Lose,
                    "matchno" => T::MatchNo,
                    "matchover" => T::MatchOver,
                    "movecontact" => T::MoveContact,
                    "moveguarded" => T::MoveGuarded,
                    "movehit" => T::MoveHit,
                    "movereversed" => T::MoveReversed,
                    "numenemy" => T::NumEnemy,
                    "numpartner" => T::NumPartner,
                    "numproj" => T::NumProj,
                    "p2life" => T::P2Life,
                    "p2stateno" => T::P2StateNo,
                    "palno" => T::PalNo,
                    "power" => T::Power,
                    "powermax" => T::PowerMax,
                    "prevstateno" => T::PrevStateNo,
                    "random" => T::Random,
                    "rightedge" => T::RightEdge,
                    "roundno" => T::RoundNo,
                    "roundsexisted" => T::RoundsExisted,
                    "roundstate" => T::RoundState,
                    "screenheight" => T::ScreenHeight,
                    "screenwidth" => T::ScreenWidth,
                    "stateno" => T::StateNo,
                    "teamside" => T::TeamSide,
                    "tickspersecond" => T::TicksPerSecond,
                    "time" | "statetime" => T::Time,
                    "topedge" => T::TopEdge,
                    "uniqhitcount" => T::UniqHitCount,
                    "win" => T::Win,
                    _ => return Err(Error::new(format!("unknown trigger {name:?}"))),
                };
                nullary(self, t)
            }
        }
    }

    /// `, OP n` (or `, n`, or `, [a,b]`) after an old-style clause; `= 0` when absent.
    /// With `required`, the comma part must be present.
    fn old_comparison(&mut self, required: bool) -> Result<()> {
        if matches!(self.peek(), Tok::Comma) {
            self.pos += 1;
            let cmp = self.cmp_op();
            if matches!(self.peek(), Tok::LBracket | Tok::LParen)
                && matches!(cmp, None | Some(Cmp::Eq) | Some(Cmp::Ne))
            {
                let lo_incl = matches!(self.next(), Tok::LBracket);
                let lo = self.integer()?;
                self.expect(Tok::Comma, "','")?;
                let hi = self.integer()?;
                let hi_incl = match self.next() {
                    Tok::RBracket => true,
                    Tok::RParen => false,
                    t => return Err(Error::new(format!("range missing ']' or ')', found {t:?}"))),
                };
                self.emit(Op::Int(lo));
                self.emit(Op::Int(hi));
                self.emit(Op::Range { lo_incl, hi_incl });
                if cmp == Some(Cmp::Ne) {
                    self.emit(Op::LNot);
                }
                return Ok(());
            }
            let n = self.integer()?;
            self.emit(Op::Int(n));
            self.emit(cmp.unwrap_or(Cmp::Eq).op());
            Ok(())
        } else if required {
            Err(Error::new("missing ', value'"))
        } else {
            self.emit(Op::Int(0));
            self.emit(Op::Eq);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::{Context, NoContext};
    use crate::value::Value;

    fn ev(src: &str) -> Value {
        compile(src)
            .unwrap_or_else(|e| panic!("{src}: {e}"))
            .eval(&mut NoContext)
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(ev("1 + 2 * 3"), Value::Int(7));
        assert_eq!(ev("(1 + 2) * 3"), Value::Int(9));
        assert_eq!(ev("7 / 2"), Value::Int(3));
        assert_eq!(ev("7.0 / 2"), Value::Float(3.5));
        assert_eq!(ev("5.0*5/6"), Value::Float(25.0 / 6.0));
        assert_eq!(ev("5/6*5.0"), Value::Float(0.0));
        assert_eq!(ev("-!0"), Value::Int(-1));
        assert_eq!(ev("2 ** 3 ** 2"), Value::Int(64), "left to right");
        // the cns.html example claims this is 1, but 0 < 1 is true, so 2 = 1 + 0 is 0
        assert_eq!(ev("1.0 = (2 = (1 > 0) + !(0 < 1))"), Value::Int(0));
        assert_eq!(ev("1.0 = (2 = (1 > 0) + (0 < 1))"), Value::Int(1));
        assert_eq!(ev("3 + 2 = 5"), Value::Int(1), "= binds looser than +");
        assert_eq!(ev("1 < 2 = 1"), Value::Int(1), "< binds tighter than =");
        assert_eq!(ev("6 & 3"), Value::Int(2));
        assert_eq!(ev("1 = 1 & 0"), Value::Int(0), "= before &");
        assert_eq!(ev("0 || 2"), Value::Int(1));
        assert_eq!(ev("1 && 0"), Value::Int(0));
        assert_eq!(ev("1 ^^ 1"), Value::Int(0));
        assert_eq!(ev("5 % 3"), Value::Int(2));
        assert_eq!(ev("abs(-2.5)"), Value::Float(2.5));
        assert_eq!(ev("floor(2.7)"), Value::Int(2));
        assert_eq!(ev("ifelse(0, 1, 2)"), Value::Int(2));
        assert_eq!(ev("cond(1, 10, 1/0)"), Value::Int(10));
        assert_eq!(ev("cond(0, 1/0, 20)"), Value::Int(20));
        assert_eq!(ev("1/0"), Value::Bottom);
        assert_eq!(
            ev("5 + 1/0"),
            Value::Int(5),
            "bottom reads as 0 in arithmetic (Ikemen)"
        );
    }

    #[test]
    fn intervals() {
        assert_eq!(ev("5 = [0, 5]"), Value::Int(1));
        assert_eq!(ev("5 = [0, 5)"), Value::Int(0));
        assert_eq!(ev("5 != (0, 5)"), Value::Int(1));
        assert_eq!(ev("(1 = [0,2]) = (0,1)"), Value::Int(0));
        assert_eq!(
            ev("2 = (1 + 1)"),
            Value::Int(1),
            "parenthesised rhs is not an interval"
        );
        assert_eq!(ev("2.5 = [2, 3]"), Value::Int(1));
    }

    struct Fake;
    impl Context for Fake {
        fn trigger(&mut self, t: &Trigger, args: &[Value]) -> Value {
            match t {
                Trigger::Time => Value::Int(7),
                Trigger::Pos(Axis::Y) => Value::Float(-3.0),
                Trigger::AnimElemTime => Value::Int(args[0].to_i() * 10),
                Trigger::Command(s) => Value::bool(s == "QCF_x"),
                Trigger::StateTypeIs { p2: false, mask } => Value::bool(mask & flags::ST_S != 0),
                Trigger::NumHelper => Value::Int(args[0].to_i()),
                Trigger::Const(n) => Value::bool(n == "velocity.walk.fwd.x"),
                Trigger::HitDefAttrIs { st, at } => Value::Int((*st | *at) as i32),
                Trigger::ProjHitTime => Value::Int(3),
                _ => Value::Bottom,
            }
        }
        fn get_var(&mut self, _: VarKind, i: i32) -> Value {
            Value::Int(i * 100)
        }
        fn set_var(&mut self, _: VarKind, _: i32, v: Value) -> Value {
            v
        }
        fn redirect(&mut self, k: Redirect, _: Option<i32>) -> bool {
            k == Redirect::Parent
        }
        fn end_redirect(&mut self) {}
    }

    fn evf(src: &str) -> Value {
        compile(src)
            .unwrap_or_else(|e| panic!("{src}: {e}"))
            .eval(&mut Fake)
    }

    #[test]
    fn triggers() {
        assert_eq!(evf("time = 7"), Value::Int(1));
        assert_eq!(evf("Pos Y < 0"), Value::Int(1));
        assert_eq!(evf("var(3)"), Value::Int(300));
        assert_eq!(evf("var(0) := 5 + 1"), Value::Int(6));
        assert_eq!(evf("(var(0) := 1) & 2"), Value::Int(0));
        assert_eq!(evf("command = \"QCF_x\""), Value::Int(1));
        assert_eq!(evf("command != \"QCF_x\""), Value::Int(0));
        assert_eq!(evf("statetype = S"), Value::Int(1));
        assert_eq!(evf("statetype != A"), Value::Int(1));
        assert_eq!(
            evf("AnimElem = 2"),
            Value::Int(0),
            "animelemtime(2)=20 != 0"
        );
        assert_eq!(evf("AnimElem = 2, = 20"), Value::Int(1));
        assert_eq!(evf("AnimElem = 2, >= 0"), Value::Int(1));
        assert_eq!(evf("AnimElem = 2, [19, 21]"), Value::Int(1));
        assert_eq!(
            evf("AnimElem = 1 + 4"),
            Value::Int(4),
            "old-style clause is one unit"
        );
        assert_eq!(evf("numhelper"), Value::Int(-1));
        assert_eq!(evf("numhelper(1234)"), Value::Int(1234));
        assert_eq!(evf("const(velocity.walk.fwd.x)"), Value::Int(1));
        assert_eq!(evf("parent, time + 1"), Value::Int(8));
        assert_eq!(evf("root, time"), Value::Bottom);
        assert_eq!(
            evf("helper(1), time > 0"),
            Value::Int(0),
            "missing redirect is bottom, bottom > 0 is false"
        );
        assert_eq!(
            evf("HitDefAttr = SC, NA, ST"),
            Value::Int((flags::ST_S | flags::ST_C | flags::AT_NA | flags::AT_ST) as i32)
        );
        assert_eq!(evf("ProjHit = 1, < 15"), Value::Int(1));
        assert_eq!(evf("ProjHit1234 = 1"), Value::Int(0));
        assert_eq!(evf("TimeMod = 4, 3"), Value::Int(1));
        assert_eq!(evf("!time"), Value::Int(0));
        assert_eq!(evf("time = [5,9] && pos y = [-5, 0]"), Value::Int(1));
    }

    #[test]
    fn fx_prefix() {
        assert_eq!(strip_fx_prefix("S1,0"), (Some('s'), "1,0"));
        assert_eq!(strip_fx_prefix("f30"), (Some('f'), "30"));
        assert_eq!(strip_fx_prefix("sysvar(0)"), (None, "sysvar(0)"));
        assert_eq!(strip_fx_prefix("facing"), (None, "facing"));
        assert_eq!(strip_fx_prefix("S time"), (Some('s'), " time"));
        assert_eq!(strip_fx_prefix("1,0"), (None, "1,0"));
        assert_eq!(strip_fx_prefix("5"), (None, "5"));
    }

    #[test]
    fn lists_and_errors() {
        assert_eq!(compile_list("1, 2 + 3, -4").unwrap().len(), 3);
        assert_eq!(compile_list("").unwrap().len(), 0);
        assert!(compile("1 +").is_err());
        assert!(compile("foo").is_err());
        assert!(compile("1 2").is_err());
    }
}
