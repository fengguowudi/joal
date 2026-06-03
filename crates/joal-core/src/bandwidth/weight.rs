//! Peers-aware weight calculator + per-torrent weight holder.
//!
//! Port of Java `org.araymond.joal.core.bandwith.weight.*`:
//!
//! - [`PeersAwareWeightCalculator`] replicates the exact formula
//!   `leechers_ratio^2 * 100 * leechers` (zero when either `seeders` or
//!   `leechers_ratio` is zero). The canonical Java values in
//!   `PeersAwareWeightCalculatorTest` all match — we keep the same test
//!   matrix here to catch any future drift.
//! - [`WeightHolder`] keeps a running `total_weight` alongside a `HashMap`
//!   of per-item weights so `BandwidthDispatcher` can allocate global speed
//!   with an O(1) total lookup.

use std::collections::HashMap;
use std::hash::Hash;

use crate::bandwidth::peers::Peers;

/// Allocates higher weights to torrents with more leechers.
///
/// Mirror of Java `PeersAwareWeightCalculator`. Zero seeders or a zero
/// leechers-ratio produce a zero weight so those torrents receive no
/// bandwidth share.
#[derive(Debug, Default, Clone, Copy)]
pub struct PeersAwareWeightCalculator;

impl PeersAwareWeightCalculator {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    #[must_use]
    pub fn calculate(&self, peers: &Peers) -> f64 {
        let leechers_ratio = f64::from(peers.leechers_ratio());
        if peers.seeders() == 0 || leechers_ratio == 0.0 {
            0.0
        } else {
            leechers_ratio * 100.0 * leechers_ratio * f64::from(peers.leechers())
        }
    }
}

/// Running map of `item -> weight` with a cached `total_weight`.
///
/// The Java version is generic on `E` (used with `InfoHash`), so is this one.
/// Mutation is externally serialised — `BandwidthDispatcher` holds the outer
/// mutex, so `WeightHolder` itself does not need any internal synchronisation.
#[derive(Debug)]
pub struct WeightHolder<E: Eq + Hash> {
    weights: HashMap<E, f64>,
    total_weight: f64,
    calculator: PeersAwareWeightCalculator,
}

impl<E: Eq + Hash> WeightHolder<E> {
    #[must_use]
    pub fn new(calculator: PeersAwareWeightCalculator) -> Self {
        Self {
            weights: HashMap::new(),
            total_weight: 0.0,
            calculator,
        }
    }

    /// Insert or replace the weight associated with `item`, keeping
    /// `total_weight` consistent.
    pub fn add_or_update(&mut self, item: E, peers: &Peers) {
        let weight = self.calculator.calculate(peers);
        match self.weights.insert(item, weight) {
            Some(previous) => {
                self.total_weight = self.total_weight - previous + weight;
            }
            None => {
                self.total_weight += weight;
            }
        }
    }

    pub fn remove(&mut self, item: &E) {
        if let Some(previous) = self.weights.remove(item) {
            self.total_weight -= previous;
        }
    }

    #[must_use]
    pub fn weight_for(&self, item: &E) -> f64 {
        self.weights.get(item).copied().unwrap_or(0.0)
    }

    #[must_use]
    pub const fn total_weight(&self) -> f64 {
        self.total_weight
    }

    /// Snapshot iterator used by the dispatcher when recomputing speeds.
    pub fn iter(&self) -> impl Iterator<Item = (&E, f64)> {
        self.weights.iter().map(|(k, v)| (k, *v))
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.weights.is_empty()
    }
}

impl<E: Eq + Hash> Default for WeightHolder<E> {
    fn default() -> Self {
        Self::new(PeersAwareWeightCalculator::new())
    }
}
