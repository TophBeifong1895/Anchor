//! A small deterministic generator (SplitMix64) so every experiment is
//! reproducible from its seed without external crates.

#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// Mix several values into one seed.
pub fn mix(parts: &[u64]) -> u64 {
    let mut rng = Rng::new(0x5EED);
    let mut acc = 0u64;
    for part in parts {
        acc ^= part.wrapping_mul(0xA24B_AED4_963E_E407);
        rng = Rng::new(acc ^ rng.next_u64());
        acc = rng.next_u64();
    }
    acc
}
