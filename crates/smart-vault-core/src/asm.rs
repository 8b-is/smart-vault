//! Low-level `aarch64` inline `asm!`, NEON SIMD, and constant-time primitives.

/// Constant-time byte-slice comparison using an anti-optimization assembly barrier on `aarch64`
/// (and `core::hint::black_box` elsewhere) so secret/hash verification resists branch folding.
#[inline]
#[must_use]
pub fn ct_eq_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    #[cfg(target_arch = "aarch64")]
    {
        let mut out = u64::from(diff);
        // SAFETY: side-effect-free register barrier (`mov` / value observation in asm) prevents
        // the compiler from short-circuiting the XOR-OR accumulation loop.
        unsafe {
            core::arch::asm!(
                "/* ct_eq_bytes barrier {0} */",
                inout(reg) out,
                options(pure, nomem, nostack, preserves_flags)
            );
        }
        out == 0
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        core::hint::black_box(diff) == 0
    }
}

/// Counts trailing zero bits in `x` using `rbit` + `clz` inline assembly on `aarch64`
/// or `trailing_zeros()` elsewhere. Returns `64` when `x == 0`.
#[inline]
#[must_use]
pub fn ctz_u64(x: u64) -> u32 {
    #[cfg(target_arch = "aarch64")]
    {
        let res: u64;
        // SAFETY: `rbit` and `clz` are pure architectural integer instructions on aarch64.
        unsafe {
            core::arch::asm!(
                "rbit {tmp}, {src}",
                "clz {out}, {tmp}",
                src = in(reg) x,
                tmp = lateout(reg) _,
                out = out(reg) res,
                options(pure, nomem, nostack, preserves_flags)
            );
        }
        res as u32
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        x.trailing_zeros()
    }
}

/// Population count (`u64::count_ones`), lowered to `cnt` + `addv` with `target-cpu=native`.
#[inline]
#[must_use]
pub const fn popcnt_u64(x: u64) -> u32 {
    x.count_ones()
}

/// Scans `haystack` for `needle` using 16-byte `aarch64` NEON vector loads (`vld1q_u8`,
/// `vceqq_u8`, `vmaxvq_u8`) with scalar tail and portable fallback.
#[inline]
#[must_use]
pub fn simd_contains_byte(haystack: &[u8], needle: u8) -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        use core::arch::aarch64::{vceqq_u8, vdupq_n_u8, vld1q_u8, vmaxvq_u8};
        let len = haystack.len();
        let mut i = 0usize;
        if len >= 16 {
            // SAFETY: `vdupq_n_u8` and aligned/unaligned `vld1q_u8` within `[i .. i + 16]`
            // are guaranteed in-bounds by the loop condition `i + 16 <= len`.
            unsafe {
                let target = vdupq_n_u8(needle);
                while i + 16 <= len {
                    let chunk = vld1q_u8(haystack.as_ptr().add(i));
                    let eq = vceqq_u8(chunk, target);
                    if vmaxvq_u8(eq) != 0 {
                        return true;
                    }
                    i += 16;
                }
            }
        }
        haystack[i..].contains(&needle)
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        haystack.contains(&needle)
    }
}

/// Lowercases ASCII uppercase bytes (`'A'..='Z'`) in `buf` in place using 16-byte NEON SIMD
/// on `aarch64` with scalar tail and portable fallback.
#[inline]
pub fn simd_ascii_lowercase_in_place(buf: &mut [u8]) {
    #[cfg(target_arch = "aarch64")]
    {
        use core::arch::aarch64::{
            vaddq_u8, vandq_u8, vcgeq_u8, vcleq_u8, vdupq_n_u8, vld1q_u8, vst1q_u8,
        };
        let len = buf.len();
        let mut i = 0usize;
        if len >= 16 {
            // SAFETY: every iteration reads and writes exactly 16 bytes inside `buf[i..i+16]`.
            unsafe {
                let lo = vdupq_n_u8(b'A');
                let hi = vdupq_n_u8(b'Z');
                let bit5 = vdupq_n_u8(0x20);
                while i + 16 <= len {
                    let ptr = buf.as_mut_ptr().add(i);
                    let v = vld1q_u8(ptr);
                    let ge_a = vcgeq_u8(v, lo);
                    let le_z = vcleq_u8(v, hi);
                    let mask = vandq_u8(ge_a, le_z);
                    let delta = vandq_u8(mask, bit5);
                    let lowered = vaddq_u8(v, delta);
                    vst1q_u8(ptr, lowered);
                    i += 16;
                }
            }
        }
        for b in &mut buf[i..] {
            if b.is_ascii_uppercase() {
                *b |= 0x20;
            }
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        for b in buf {
            if b.is_ascii_uppercase() {
                *b |= 0x20;
            }
        }
    }
}

/// Zero-allocation 64-bit FNV-1a hash.
#[inline]
#[must_use]
pub const fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
        i += 1;
    }
    hash
}
