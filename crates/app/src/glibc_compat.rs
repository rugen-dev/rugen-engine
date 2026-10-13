//! Keeps the Linux binary runnable on older glibc.
//!
//! glibc 2.43 gave `acosf`, `asinf`, `acoshf`, `coshf`, `sinhf` and
//! `log10f` new symbol versions (correctly rounded), so a binary linked on a
//! 2.43 system (Fedora 44, where releases are built) asks for
//! `GLIBC_2.43` and refuses to start on every older distribution. Defining
//! them here makes the linker resolve them in the binary instead of
//! importing them from libm. They come from the pure-Rust `libm` crate (the
//! simulation already uses it for its own math, so nothing deterministic
//! depends on these); only dependencies (audio, GPU stack) call them.
#![allow(unsafe_code)]

#[unsafe(no_mangle)]
pub extern "C" fn acosf(x: f32) -> f32 {
    libm::acosf(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn asinf(x: f32) -> f32 {
    libm::asinf(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn acoshf(x: f32) -> f32 {
    libm::acoshf(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn coshf(x: f32) -> f32 {
    libm::coshf(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn sinhf(x: f32) -> f32 {
    libm::sinhf(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn log10f(x: f32) -> f32 {
    libm::log10f(x)
}

#[cfg(test)]
mod tests {
    #[test]
    fn known_values() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
        assert!(close(super::acosf(0.0), core::f32::consts::FRAC_PI_2));
        assert!(close(super::asinf(1.0), core::f32::consts::FRAC_PI_2));
        assert!(close(super::acoshf(1.0), 0.0));
        assert!(close(super::coshf(0.0), 1.0));
        assert!(close(super::sinhf(0.0), 0.0));
        assert!(close(super::log10f(1000.0), 3.0));
    }
}
