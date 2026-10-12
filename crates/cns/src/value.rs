//! MUGEN values: 32-bit int, 32-bit float, or bottom (error / undefined).
//!
//! Promotion: an op with any float operand is a float op. Bottom reads as 0
//! in arithmetic and comparisons (Ikemen GO behavior), stays bottom through
//! `!` and interval checks, and is false as a condition.

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Int(i32),
    Float(f32),
    Bottom,
}

// The arithmetic methods deliberately mirror MUGEN operator names, not std traits:
// they take and return `Value` with MUGEN promotion rules, not Rust's.
#[allow(clippy::should_implement_trait)]
impl Value {
    pub const TRUE: Value = Value::Int(1);
    pub const FALSE: Value = Value::Int(0);

    pub fn bool(b: bool) -> Value {
        if b { Value::TRUE } else { Value::FALSE }
    }

    pub fn float(f: f32) -> Value {
        if f.is_nan() {
            Value::Bottom
        } else {
            Value::Float(f)
        }
    }

    pub fn is_bottom(self) -> bool {
        matches!(self, Value::Bottom)
    }
    pub fn is_float(self) -> bool {
        matches!(self, Value::Float(_))
    }

    pub fn to_f(self) -> f32 {
        match self {
            Value::Int(i) => i as f32,
            Value::Float(f) => f,
            Value::Bottom => 0.0,
        }
    }
    /// Truncates floats toward zero like a C cast.
    pub fn to_i(self) -> i32 {
        match self {
            Value::Int(i) => i,
            Value::Float(f) => f as i32,
            Value::Bottom => 0,
        }
    }
    pub fn to_b(self) -> bool {
        match self {
            Value::Int(i) => i != 0,
            Value::Float(f) => f != 0.0,
            Value::Bottom => false,
        }
    }

    fn either_float(a: Value, b: Value) -> bool {
        a.is_float() || b.is_float()
    }

    pub fn neg(self) -> Value {
        match self {
            Value::Float(f) => Value::Float(-f),
            v => Value::Int(v.to_i().wrapping_neg()),
        }
    }
    pub fn bit_not(self) -> Value {
        Value::Int(!self.to_i())
    }
    pub fn lnot(self) -> Value {
        if self.is_bottom() {
            Value::Bottom
        } else {
            Value::bool(!self.to_b())
        }
    }

    pub fn add(self, o: Value) -> Value {
        if Self::either_float(self, o) {
            Value::float(self.to_f() + o.to_f())
        } else {
            Value::Int(self.to_i().wrapping_add(o.to_i()))
        }
    }
    pub fn sub(self, o: Value) -> Value {
        if Self::either_float(self, o) {
            Value::float(self.to_f() - o.to_f())
        } else {
            Value::Int(self.to_i().wrapping_sub(o.to_i()))
        }
    }
    pub fn mul(self, o: Value) -> Value {
        if Self::either_float(self, o) {
            Value::float(self.to_f() * o.to_f())
        } else {
            Value::Int(self.to_i().wrapping_mul(o.to_i()))
        }
    }
    pub fn div(self, o: Value) -> Value {
        if o.to_f() == 0.0 {
            Value::Bottom
        } else if Self::either_float(self, o) {
            Value::float(self.to_f() / o.to_f())
        } else {
            Value::Int(self.to_i().wrapping_div(o.to_i()))
        }
    }
    pub fn rem(self, o: Value) -> Value {
        if o.to_i() == 0 {
            Value::Bottom
        } else {
            Value::Int(self.to_i().wrapping_rem(o.to_i()))
        }
    }
    pub fn pow(self, o: Value) -> Value {
        if Self::either_float(self, o) || o.to_f() < 0.0 {
            Value::float(libm::powf(self.to_f(), o.to_f()))
        } else {
            // binary exponentiation, wrapping like Ikemen
            let (mut base, mut e, mut r) = (self.to_i(), o.to_i() as u32, 1i32);
            while e > 0 {
                if e & 1 != 0 {
                    r = r.wrapping_mul(base);
                }
                base = base.wrapping_mul(base);
                e >>= 1;
            }
            Value::Int(r)
        }
    }

    pub fn lt(self, o: Value) -> Value {
        Value::bool(if Self::either_float(self, o) {
            self.to_f() < o.to_f()
        } else {
            self.to_i() < o.to_i()
        })
    }
    pub fn le(self, o: Value) -> Value {
        Value::bool(if Self::either_float(self, o) {
            self.to_f() <= o.to_f()
        } else {
            self.to_i() <= o.to_i()
        })
    }
    pub fn gt(self, o: Value) -> Value {
        Value::bool(if Self::either_float(self, o) {
            self.to_f() > o.to_f()
        } else {
            self.to_i() > o.to_i()
        })
    }
    pub fn ge(self, o: Value) -> Value {
        Value::bool(if Self::either_float(self, o) {
            self.to_f() >= o.to_f()
        } else {
            self.to_i() >= o.to_i()
        })
    }
    pub fn eq(self, o: Value) -> Value {
        Value::bool(if Self::either_float(self, o) {
            self.to_f() == o.to_f()
        } else {
            self.to_i() == o.to_i()
        })
    }
    pub fn ne(self, o: Value) -> Value {
        Value::bool(if Self::either_float(self, o) {
            self.to_f() != o.to_f()
        } else {
            self.to_i() != o.to_i()
        })
    }

    /// `self` in the interval `lo..hi` with the given inclusivity. Bottom anywhere is bottom.
    pub fn in_range(self, lo: Value, hi: Value, lo_incl: bool, hi_incl: bool) -> Value {
        if self.is_bottom() || lo.is_bottom() || hi.is_bottom() {
            return Value::Bottom;
        }
        let (a, b) = if self.is_float() || lo.is_float() || hi.is_float() {
            let x = self.to_f();
            (
                if lo_incl {
                    x >= lo.to_f()
                } else {
                    x > lo.to_f()
                },
                if hi_incl {
                    x <= hi.to_f()
                } else {
                    x < hi.to_f()
                },
            )
        } else {
            let x = self.to_i();
            (
                if lo_incl {
                    x >= lo.to_i()
                } else {
                    x > lo.to_i()
                },
                if hi_incl {
                    x <= hi.to_i()
                } else {
                    x < hi.to_i()
                },
            )
        };
        Value::bool(a && b)
    }

    pub fn bit_and(self, o: Value) -> Value {
        Value::Int(self.to_i() & o.to_i())
    }
    pub fn bit_or(self, o: Value) -> Value {
        Value::Int(self.to_i() | o.to_i())
    }
    pub fn bit_xor(self, o: Value) -> Value {
        Value::Int(self.to_i() ^ o.to_i())
    }
    pub fn lxor(self, o: Value) -> Value {
        Value::bool(self.to_b() != o.to_b())
    }

    pub fn abs(self) -> Value {
        match self {
            Value::Float(f) => Value::Float(f.abs()),
            v => Value::Int(v.to_i().wrapping_abs()),
        }
    }
    pub fn floor(self) -> Value {
        match self {
            Value::Float(f) => Value::Int(libm::floorf(f) as i32),
            v => v,
        }
    }
    pub fn ceil(self) -> Value {
        match self {
            Value::Float(f) => Value::Int(libm::ceilf(f) as i32),
            v => v,
        }
    }
    pub fn exp(self) -> Value {
        Value::float(libm::expf(self.to_f()))
    }
    pub fn ln(self) -> Value {
        let f = self.to_f();
        if f <= 0.0 {
            Value::Bottom
        } else {
            Value::float(libm::logf(f))
        }
    }
    /// `log(base, x)`
    pub fn log(self, x: Value) -> Value {
        let (b, x) = (self.to_f(), x.to_f());
        if b <= 0.0 || x <= 0.0 || b == 1.0 {
            Value::Bottom
        } else {
            Value::float(libm::logf(x) / libm::logf(b))
        }
    }
    pub fn sin(self) -> Value {
        Value::float(libm::sinf(self.to_f()))
    }
    pub fn cos(self) -> Value {
        Value::float(libm::cosf(self.to_f()))
    }
    pub fn tan(self) -> Value {
        Value::float(libm::tanf(self.to_f()))
    }
    pub fn asin(self) -> Value {
        Value::float(libm::asinf(self.to_f()))
    }
    pub fn acos(self) -> Value {
        Value::float(libm::acosf(self.to_f()))
    }
    pub fn atan(self) -> Value {
        Value::float(libm::atanf(self.to_f()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn promotion() {
        assert_eq!(Value::Int(7).div(Value::Int(2)), Value::Int(3));
        assert_eq!(Value::Float(7.0).div(Value::Int(2)), Value::Float(3.5));
        assert_eq!(Value::Int(7).div(Value::Int(0)), Value::Bottom);
        assert_eq!(Value::Int(2).pow(Value::Int(10)), Value::Int(1024));
        assert_eq!(Value::Int(0).pow(Value::Int(0)), Value::Int(1));
        assert_eq!(Value::Int(2).pow(Value::Int(-1)), Value::Float(0.5));
        assert_eq!(Value::Int(1).eq(Value::Float(1.0)), Value::TRUE);
        assert_eq!(Value::Bottom.lnot(), Value::Bottom);
        assert_eq!(
            Value::Int(5).in_range(Value::Int(0), Value::Int(5), true, false),
            Value::FALSE
        );
        assert_eq!(Value::Float(-3.5).floor(), Value::Int(-4));
    }
}
