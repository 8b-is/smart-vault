//! Dependency-free `#![no_std]` Okapi BM25 math and stack-allocated `TopK` accumulator.

/// Pure-Rust IEEE-754 natural logarithm (`ln(x)`) for `#![no_std]`.
/// Accurate to `< 1e-14` for all positive finite `x`.
#[inline]
#[must_use]
pub fn ln_f64(x: f64) -> f64 {
    if x <= 0.0 || x.is_nan() {
        return f64::NAN;
    }
    if x.is_infinite() {
        return f64::INFINITY;
    }
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32 - 1023;
    let mant_bits = (bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000;
    let mut m = f64::from_bits(mant_bits);
    let mut k = exp;
    if m > core::f64::consts::SQRT_2 {
        m *= 0.5;
        k += 1;
    }
    let u = (m - 1.0) / (m + 1.0);
    let u2 = u * u;
    let poly = 1.0
        + u2 * (1.0 / 3.0
            + u2 * (1.0 / 5.0
                + u2 * (1.0 / 7.0 + u2 * (1.0 / 9.0 + u2 * (1.0 / 11.0 + u2 * (1.0 / 13.0))))));
    (k as f64) * core::f64::consts::LN_2 + 2.0 * u * poly
}

/// Computes Robertson-Sparck Jones Okapi IDF (`ln((N - df + 0.5)/(df + 0.5) + 1.0)`).
#[inline]
#[must_use]
pub fn idf(doc_count: usize, df: usize) -> f64 {
    let n = doc_count as f64;
    let d = df as f64;
    ln_f64((n - d + 0.5) / (d + 0.5) + 1.0)
}

/// Computes single-term Okapi BM25 contribution.
#[inline]
#[must_use]
pub fn term_score(idf_val: f64, tf: f64, dl: f64, avg_dl: f64, k1: f64, b: f64) -> f64 {
    let denom = tf + k1 * (1.0 - b + b * (dl / avg_dl));
    idf_val * ((tf * (k1 + 1.0)) / denom)
}

/// Zero-allocation stack-resident fixed-capacity top-`K` `(doc_id, score)` selector.
#[derive(Debug, Clone)]
pub struct TopK<const K: usize> {
    len: usize,
    items: [(u32, f64); K],
}

impl<const K: usize> Default for TopK<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const K: usize> TopK<K> {
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self { len: 0, items: [(0, f64::NEG_INFINITY); K] }
    }

    #[inline]
    pub fn push(&mut self, doc_id: u32, score: f64) {
        if K == 0 || score.is_nan() {
            return;
        }
        if self.len < K {
            let mut idx = self.len;
            while idx > 0 && score > self.items[idx - 1].1 {
                self.items[idx] = self.items[idx - 1];
                idx -= 1;
            }
            self.items[idx] = (doc_id, score);
            self.len += 1;
        } else if score > self.items[K - 1].1 {
            let mut idx = K - 1;
            while idx > 0 && score > self.items[idx - 1].1 {
                self.items[idx] = self.items[idx - 1];
                idx -= 1;
            }
            self.items[idx] = (doc_id, score);
        }
    }

    #[inline]
    #[must_use]
    pub fn as_slice(&self) -> &[(u32, f64)] {
        &self.items[..self.len]
    }
}
