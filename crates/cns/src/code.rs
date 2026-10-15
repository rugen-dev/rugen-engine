//! Stack bytecode and its VM.

use crate::trigger::Trigger;
use crate::value::Value;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VarKind {
    Var,
    FVar,
    SysVar,
    SysFVar,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Redirect {
    Parent,
    Root,
    Helper,
    Target,
    Partner,
    Enemy,
    EnemyNear,
    PlayerId,
    P2,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub enum Op {
    Int(i32),
    Float(f32),
    Neg,
    BitNot,
    LNot,
    /// Coerce top to 0/1.
    ToBool,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    BitAnd,
    BitOr,
    BitXor,
    LXor,
    /// pops hi, lo, x ; pushes x in [lo,hi] with inclusivity flags
    Range {
        lo_incl: bool,
        hi_incl: bool,
    },
    /// relative jumps: offset from the op after this one
    Jz(u32),
    Jnz(u32),
    Jmp(u32),
    /// Jump if top is bottom (keeps it).
    Jbottom(u32),
    Pop,
    Abs,
    Floor,
    Ceil,
    Exp,
    Ln,
    /// pops x, base
    Log,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    /// pops false, true, cond
    IfElse,
    /// pops index
    GetVar(VarKind),
    /// pops value, index ; pushes value
    SetVar(VarKind),
    Trigger(Trigger),
    /// Evaluate the next `len` ops as the redirect target. If `with_id`, pops the id first.
    /// Missing target: push bottom and skip.
    Redirect {
        kind: Redirect,
        with_id: bool,
        len: u32,
    },
    /// Old-style `ProjHit = v, OP n`: pops t (ticks since event, -1 = never),
    /// pushes `expect == (t >= 0 && t OP value)`.
    ProjClause {
        cmp: Cmp,
        value: i32,
        expect: bool,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    pub fn apply(self, a: Value, b: Value) -> Value {
        match self {
            Cmp::Eq => a.eq(b),
            Cmp::Ne => a.ne(b),
            Cmp::Lt => a.lt(b),
            Cmp::Le => a.le(b),
            Cmp::Gt => a.gt(b),
            Cmp::Ge => a.ge(b),
        }
    }
    pub fn op(self) -> Op {
        match self {
            Cmp::Eq => Op::Eq,
            Cmp::Ne => Op::Ne,
            Cmp::Lt => Op::Lt,
            Cmp::Le => Op::Le,
            Cmp::Gt => Op::Gt,
            Cmp::Ge => Op::Ge,
        }
    }
}

/// A compiled expression.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Code {
    pub ops: Vec<Op>,
}

/// What the VM asks the simulation for.
pub trait Context {
    fn trigger(&mut self, t: &Trigger, args: &[Value]) -> Value;
    fn get_var(&mut self, kind: VarKind, index: i32) -> Value;
    fn set_var(&mut self, kind: VarKind, index: i32, value: Value) -> Value;
    /// Make `kind`/`id` the current entity for subsequent calls. `false` if it does not exist.
    fn redirect(&mut self, kind: Redirect, id: Option<i32>) -> bool;
    fn end_redirect(&mut self);
}

impl Code {
    /// Constant-only expression value, if it has no triggers or variables.
    pub fn as_const(&self) -> Option<Value> {
        let mut nc = NoContext;
        if self.ops.iter().any(|o| {
            matches!(
                o,
                Op::Trigger(_) | Op::GetVar(_) | Op::SetVar(_) | Op::Redirect { .. }
            )
        }) {
            return None;
        }
        Some(self.eval(&mut nc))
    }

    pub fn eval<C: Context>(&self, ctx: &mut C) -> Value {
        // The value stack comes from a per-thread pool, so a frame's
        // thousands of trigger evaluations allocate nothing; taking one (not
        // borrowing a single shared stack) keeps re-entrant evaluations
        // (a trigger that evaluates another expression) independent.
        let mut stack = STACKS.with(|p| p.borrow_mut().pop()).unwrap_or_default();
        stack.clear();
        self.run(ctx, &mut stack, 0, self.ops.len());
        let out = stack.pop().unwrap_or(Value::Bottom);
        STACKS.with(|p| {
            let mut p = p.borrow_mut();
            if p.len() < 8 {
                p.push(stack);
            }
        });
        out
    }

    pub fn eval_b<C: Context>(&self, ctx: &mut C) -> bool {
        self.eval(ctx).to_b()
    }
    pub fn eval_i<C: Context>(&self, ctx: &mut C) -> i32 {
        self.eval(ctx).to_i()
    }
    pub fn eval_f<C: Context>(&self, ctx: &mut C) -> f32 {
        self.eval(ctx).to_f()
    }

    fn run<C: Context>(&self, ctx: &mut C, st: &mut Vec<Value>, start: usize, end: usize) {
        let mut pc = start;
        macro_rules! pop {
            () => {
                st.pop().unwrap_or(Value::Bottom)
            };
        }
        macro_rules! bin {
            ($f:ident) => {{
                let b = pop!();
                let a = pop!();
                st.push(a.$f(b));
            }};
        }
        macro_rules! un {
            ($f:ident) => {{
                let a = pop!();
                st.push(a.$f());
            }};
        }
        while pc < end {
            let op = &self.ops[pc];
            pc += 1;
            match op {
                Op::Int(i) => st.push(Value::Int(*i)),
                Op::Float(f) => st.push(Value::Float(*f)),
                Op::Neg => un!(neg),
                Op::BitNot => un!(bit_not),
                Op::LNot => un!(lnot),
                Op::ToBool => {
                    let a = pop!();
                    st.push(if a.is_bottom() {
                        Value::Bottom
                    } else {
                        Value::bool(a.to_b())
                    });
                }
                Op::Add => bin!(add),
                Op::Sub => bin!(sub),
                Op::Mul => bin!(mul),
                Op::Div => bin!(div),
                Op::Mod => bin!(rem),
                Op::Pow => bin!(pow),
                Op::Lt => bin!(lt),
                Op::Le => bin!(le),
                Op::Gt => bin!(gt),
                Op::Ge => bin!(ge),
                Op::Eq => bin!(eq),
                Op::Ne => bin!(ne),
                Op::BitAnd => bin!(bit_and),
                Op::BitOr => bin!(bit_or),
                Op::BitXor => bin!(bit_xor),
                Op::LXor => bin!(lxor),
                Op::Range { lo_incl, hi_incl } => {
                    let hi = pop!();
                    let lo = pop!();
                    let x = pop!();
                    st.push(x.in_range(lo, hi, *lo_incl, *hi_incl));
                }
                Op::Jz(n) => {
                    if !st.last().copied().unwrap_or(Value::Bottom).to_b() {
                        pc += *n as usize;
                    }
                }
                Op::Jnz(n) => {
                    if st.last().copied().unwrap_or(Value::Bottom).to_b() {
                        pc += *n as usize;
                    }
                }
                Op::Jmp(n) => pc += *n as usize,
                Op::Jbottom(n) => {
                    if st.last().copied().unwrap_or(Value::Bottom).is_bottom() {
                        pc += *n as usize;
                    }
                }
                Op::Pop => {
                    st.pop();
                }
                Op::Abs => un!(abs),
                Op::Floor => un!(floor),
                Op::Ceil => un!(ceil),
                Op::Exp => un!(exp),
                Op::Ln => un!(ln),
                Op::Log => bin!(log),
                Op::Sin => un!(sin),
                Op::Cos => un!(cos),
                Op::Tan => un!(tan),
                Op::Asin => un!(asin),
                Op::Acos => un!(acos),
                Op::Atan => un!(atan),
                Op::IfElse => {
                    let f = pop!();
                    let t = pop!();
                    let c = pop!();
                    st.push(if c.is_bottom() {
                        Value::Bottom
                    } else if c.to_b() {
                        t
                    } else {
                        f
                    });
                }
                Op::GetVar(k) => {
                    let i = pop!();
                    st.push(ctx.get_var(*k, i.to_i()));
                }
                Op::SetVar(k) => {
                    let v = pop!();
                    let i = pop!();
                    st.push(ctx.set_var(*k, i.to_i(), v));
                }
                Op::Trigger(t) => {
                    // triggers take at most one argument (`Trigger::arity`)
                    let n = t.arity().min(1);
                    let at = st.len().saturating_sub(n);
                    let arg = [st.get(at).copied().unwrap_or(Value::Bottom)];
                    st.truncate(at);
                    st.push(ctx.trigger(t, &arg[..n]));
                }
                Op::Redirect { kind, with_id, len } => {
                    let id = if *with_id { Some(pop!().to_i()) } else { None };
                    let len = *len as usize;
                    if ctx.redirect(*kind, id) {
                        self.run(ctx, st, pc, pc + len);
                        ctx.end_redirect();
                    } else {
                        st.push(Value::Bottom);
                    }
                    pc += len;
                }
                Op::ProjClause { cmp, value, expect } => {
                    let t = pop!();
                    let hit =
                        !t.is_bottom() && t.to_i() >= 0 && cmp.apply(t, Value::Int(*value)).to_b();
                    st.push(Value::bool(hit == *expect));
                }
            }
        }
    }
}

thread_local! {
    /// Value stacks kept between evaluations (see [`Code::eval`]).
    static STACKS: std::cell::RefCell<Vec<Vec<Value>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Context with no simulation behind it: every trigger is bottom.
pub struct NoContext;
impl Context for NoContext {
    fn trigger(&mut self, _: &Trigger, _: &[Value]) -> Value {
        Value::Bottom
    }
    fn get_var(&mut self, _: VarKind, _: i32) -> Value {
        Value::Bottom
    }
    fn set_var(&mut self, _: VarKind, _: i32, v: Value) -> Value {
        v
    }
    fn redirect(&mut self, _: Redirect, _: Option<i32>) -> bool {
        false
    }
    fn end_redirect(&mut self) {}
}
