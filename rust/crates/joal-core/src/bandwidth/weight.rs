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

#[cfg(test)]
mod tests {
    use super::*;

    // --- PeersAwareWeightCalculator: matches Java `PeersAwareWeightCalculatorTest` ---

    #[test]
    fn calculator_never_goes_below_zero_for_degenerate_cases() {
        let c = PeersAwareWeightCalculator::new();
        assert_eq!(c.calculate(&Peers::new(0, 0)), 0.0);
        assert_eq!(c.calculate(&Peers::new(0, 1)), 0.0);
        assert_eq!(c.calculate(&Peers::new(1, 0)), 0.0);
    }

    #[test]
    fn calculator_promotes_torrents_with_more_leechers() {
        let c = PeersAwareWeightCalculator::new();
        let first = c.calculate(&Peers::new(10, 10));
        let second = c.calculate(&Peers::new(10, 30));
        let third = c.calculate(&Peers::new(10, 100));
        let fourth = c.calculate(&Peers::new(10, 200));
        assert!(fourth > third);
        assert!(third > second);
        assert!(second > first);
    }

    #[test]
    fn calculator_exact_values_for_canonical_java_matrix() {
        let c = PeersAwareWeightCalculator::new();
        // Matches `PeersAwareWeightCalculatorTest.shouldProvideExactValues`.
        assert!((c.calculate(&Peers::new(1, 1)) - 25.0).abs() < 1e-6);
        assert!((c.calculate(&Peers::new(2, 1)) - 11.1).abs() < 0.1);
        assert!((c.calculate(&Peers::new(30, 1)) - 0.104_058_273).abs() < 1e-6);
        assert_eq!(c.calculate(&Peers::new(0, 1)), 0.0);
        assert_eq!(c.calculate(&Peers::new(1, 0)), 0.0);
        assert!((c.calculate(&Peers::new(2, 100)) - 9_611.687_812).abs() < 1e-3);
        assert_eq!(c.calculate(&Peers::new(0, 100)), 0.0);
        assert!((c.calculate(&Peers::new(2000, 150)) - 73.012_439_16).abs() < 1e-3);
        assert!((c.calculate(&Peers::new(150, 2000)) - 173_066.522_4).abs() < 0.1);
        assert!((c.calculate(&Peers::new(80, 2000)) - 184_911.242_6).abs() < 0.5);
        assert!((c.calculate(&Peers::new(2000, 2000)) - 50_000.0).abs() < 1e-6);
        assert_eq!(c.calculate(&Peers::new(0, 0)), 0.0);
    }

    // --- WeightHolder: matches Java `WeightHolderTest` ---

    #[test]
    fn holder_returns_zero_for_missing_item() {
        let holder = WeightHolder::<&'static str>::default();
        assert_eq!(holder.weight_for(&"q"), 0.0);
    }

    #[test]
    fn holder_stores_weight_for_added_item() {
        let mut holder = WeightHolder::<&'static str>::default();
        holder.add_or_update("a", &Peers::new(10, 10));
        // 0.5^2 * 100 * 10 = 250
        assert!((holder.weight_for(&"a") - 250.0).abs() < 1e-6);
    }

    #[test]
    fn holder_tracks_total_weight_across_add_update_remove() {
        let mut holder = WeightHolder::<&'static str>::default();

        // add "a": Peers(1,1) -> 25.0
        holder.add_or_update("a", &Peers::new(1, 1));
        assert!((holder.total_weight() - 25.0).abs() < 1e-6);

        // add "b": Peers(2,2) -> 0.5^2 * 100 * 2 = 50.0 → total 75.0
        holder.add_or_update("b", &Peers::new(2, 2));
        assert!((holder.total_weight() - 75.0).abs() < 1e-6);

        // update "b" to Peers(10,10) -> 250.0 → total 25 + 250 = 275.0
        holder.add_or_update("b", &Peers::new(10, 10));
        assert!((holder.total_weight() - 275.0).abs() < 1e-6);

        // remove "a" → total 250.0
        holder.remove(&"a");
        assert!((holder.total_weight() - 250.0).abs() < 1e-6);
    }

    #[test]
    fn holder_remove_missing_is_noop() {
        let mut holder = WeightHolder::<&'static str>::default();
        holder.remove(&"missing");
        assert_eq!(holder.total_weight(), 0.0);
    }
}
