//! Thread-level hardware Flush-To-Zero (FTZ) and Denormals-Are-Zero (DAZ) controls (`Design.md §4.1`).
//!
//! Subnormal floating point values in upstream FIR filter delay lines cause severe CPU latency spikes.
//! Setting FTZ and DAZ ensures any underflowing subnormal decays immediately to 0.0 without microcode traps.

/// RAII guard that enables hardware FTZ/DAZ upon creation and restores the previous state on drop.
#[derive(Debug)]
pub struct ScopedFtzDaz {
    #[cfg(target_arch = "x86_64")]
    prev_mxcsr: u32,
    #[cfg(target_arch = "aarch64")]
    prev_fpcr: u64,
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    _unused: (),
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn get_mxcsr() -> u32 {
    let mut mxcsr: u32 = 0;
    unsafe {
        core::arch::asm!("stmxcsr [{}]", in(reg) &mut mxcsr, options(nostack, preserves_flags));
    }
    mxcsr
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn set_mxcsr(val: u32) {
    let mxcsr = val;
    unsafe {
        core::arch::asm!("ldmxcsr [{}]", in(reg) &mxcsr, options(nostack, preserves_flags));
    }
}

impl ScopedFtzDaz {
    /// Enables FTZ and DAZ on the current thread, returning an RAII guard that restores previous flags on drop.
    pub fn new() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            // Bit 15: FTZ (Flush-to-zero)
            // Bit 6:  DAZ (Denormals-are-zero)
            const FTZ_DAZ_MASK: u32 = (1 << 15) | (1 << 6);
            let prev_mxcsr = unsafe { get_mxcsr() };
            unsafe { set_mxcsr(prev_mxcsr | FTZ_DAZ_MASK) };
            Self { prev_mxcsr }
        }

        #[cfg(target_arch = "aarch64")]
        {
            // Bit 24: FZ (Flush-to-zero)
            const FZ_MASK: u64 = 1 << 24;
            let mut prev_fpcr: u64;
            unsafe {
                core::arch::asm!("mrs {}, fpcr", out(reg) prev_fpcr);
                core::arch::asm!("msr fpcr, {}", in(reg) (prev_fpcr | FZ_MASK));
            }
            Self { prev_fpcr }
        }

        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Self { _unused: () }
        }
    }

    /// Returns `true` if FTZ/DAZ is supported and active in hardware on this platform.
    pub fn is_active() -> bool {
        #[cfg(target_arch = "x86_64")]
        {
            const FTZ_DAZ_MASK: u32 = (1 << 15) | (1 << 6);
            let mxcsr = unsafe { get_mxcsr() };
            (mxcsr & FTZ_DAZ_MASK) == FTZ_DAZ_MASK
        }

        #[cfg(target_arch = "aarch64")]
        {
            const FZ_MASK: u64 = 1 << 24;
            let fpcr: u64;
            unsafe {
                core::arch::asm!("mrs {}, fpcr", out(reg) fpcr);
            }
            (fpcr & FZ_MASK) == FZ_MASK
        }

        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            false
        }
    }
}

impl Default for ScopedFtzDaz {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ScopedFtzDaz {
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        {
            unsafe { set_mxcsr(self.prev_mxcsr) };
        }

        #[cfg(target_arch = "aarch64")]
        {
            unsafe {
                core::arch::asm!("msr fpcr, {}", in(reg) self.prev_fpcr);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scoped_ftz_daz_lifecycle() {
        let initial_active = ScopedFtzDaz::is_active();
        {
            let _guard = ScopedFtzDaz::new();
            if cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) {
                assert!(ScopedFtzDaz::is_active());
            }
        }
        // Should restore to initial state
        assert_eq!(ScopedFtzDaz::is_active(), initial_active);
    }
}
