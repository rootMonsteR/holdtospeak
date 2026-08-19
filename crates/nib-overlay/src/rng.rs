//! Deterministic visual noise source for the animated styles.

/// xorshift64* — visual noise only. Fixed seed on purpose: identical dumps across runs make
/// design iteration diffable (only const edits change the output).
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    pub(crate) fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0 // [0, 1)
    }

    pub(crate) fn signed(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }

    pub(crate) fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next_f32() * (hi - lo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_unit_interval() {
        let mut rng = Rng::new(42);
        for _ in 0..10_000 {
            let v = rng.next_f32();
            assert!((0.0..1.0).contains(&v));
        }
    }
}
