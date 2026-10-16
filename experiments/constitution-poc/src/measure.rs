//! Measurement: streaming probes and the statistics a deterministic gate needs.
//!
//! Every adjudication produces one point at constant cost. Aggregation is
//! counting; no model reads any record.

use std::collections::BTreeMap;

use crate::kernel::{Effect, Point, Reason};
use crate::rng::Rng;

/// Streaming counters. Memory grows with the number of distinct keys, never
/// with the number of events.
#[derive(Clone, Debug, Default)]
pub struct Aggregator {
    pub events: u64,
    pub by_reason: BTreeMap<(String, Reason), u64>,
    pub by_effect: BTreeMap<(String, Effect), u64>,
    pub cost: BTreeMap<String, u64>,
}

impl Aggregator {
    pub fn observe(&mut self, point: &Point) {
        self.events += 1;
        *self
            .by_reason
            .entry((point.component.clone(), point.reason))
            .or_default() += 1;
        *self
            .by_effect
            .entry((point.component.clone(), point.effect))
            .or_default() += 1;
        *self.cost.entry(point.component.clone()).or_default() += point.cost;
    }

    pub fn keys(&self) -> usize {
        self.by_reason.len() + self.by_effect.len() + self.cost.len()
    }

    /// Reasons ranked by frequency: the walls hit most often.
    pub fn top_reasons(&self, n: usize) -> Vec<((String, Reason), u64)> {
        let mut all: Vec<_> = self
            .by_reason
            .iter()
            .filter(|((_, r), _)| *r != Reason::Ok)
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        all.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        all.truncate(n);
        all
    }
}

pub fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

/// Paired sign-flip permutation test on per-case differences of a statistic
/// computed over all cases. `stat(a, b)` must be large when B is better.
/// Returns the one-sided p-value. Assumes nothing about distributions.
pub fn paired_permutation<T: Clone>(
    a: &[T],
    b: &[T],
    stat: impl Fn(&[T], &[T]) -> f64,
    resamples: u32,
    seed: u64,
) -> (f64, f64) {
    assert_eq!(a.len(), b.len());
    let observed = stat(a, b);
    let mut rng = Rng::new(seed);
    let mut at_least = 0u32;
    let mut left = Vec::with_capacity(a.len());
    let mut right = Vec::with_capacity(b.len());
    for _ in 0..resamples {
        left.clear();
        right.clear();
        for (x, y) in a.iter().zip(b) {
            if rng.chance(0.5) {
                left.push(y.clone());
                right.push(x.clone());
            } else {
                left.push(x.clone());
                right.push(y.clone());
            }
        }
        if stat(&left, &right) >= observed {
            at_least += 1;
        }
    }
    (observed, (at_least as f64 + 1.0) / (resamples as f64 + 1.0))
}

/// Spearman rank correlation, no ties expected.
pub fn spearman(x: &[f64], y: &[f64]) -> f64 {
    fn ranks(v: &[f64]) -> Vec<f64> {
        let mut idx: Vec<usize> = (0..v.len()).collect();
        idx.sort_by(|&i, &j| v[i].partial_cmp(&v[j]).unwrap());
        let mut r = vec![0.0; v.len()];
        for (rank, &i) in idx.iter().enumerate() {
            r[i] = rank as f64;
        }
        r
    }
    let (rx, ry) = (ranks(x), ranks(y));
    let n = x.len() as f64;
    let d2: f64 = rx.iter().zip(&ry).map(|(a, b)| (a - b).powi(2)).sum();
    1.0 - 6.0 * d2 / (n * (n * n - 1.0))
}
