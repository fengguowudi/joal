//! `(seeders, leechers)` peer-count pair + pre-computed leecher ratio.
//!
//! Port of Java `org.araymond.joal.core.bandwith.Peers`. The Java class uses
//! `@EqualsAndHashCode(of = {"seeders","leechers"})` so equality ignores the
//! derived `leechersRatio`; we replicate that by deriving only on the
//! primitive fields and recomputing the ratio.

/// Snapshot of the peer population for one torrent.
///
/// `leechers_ratio` = `leechers / (seeders + leechers)` in `f32` precision,
/// matching the Java `float` field. Equality and hashing consider only
/// `seeders` and `leechers`; the ratio is derived.
#[derive(Debug, Clone, Copy)]
pub struct Peers {
    seeders: u32,
    leechers: u32,
    leechers_ratio: f32,
}

impl Peers {
    #[must_use]
    pub fn new(seeders: u32, leechers: u32) -> Self {
        let all_peers = seeders.saturating_add(leechers);
        let leechers_ratio = if all_peers == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let r = (leechers as f32) / (all_peers as f32);
            r
        };
        Self {
            seeders,
            leechers,
            leechers_ratio,
        }
    }

    #[must_use]
    pub const fn seeders(&self) -> u32 {
        self.seeders
    }

    #[must_use]
    pub const fn leechers(&self) -> u32 {
        self.leechers
    }

    #[must_use]
    pub const fn leechers_ratio(&self) -> f32 {
        self.leechers_ratio
    }
}

impl PartialEq for Peers {
    fn eq(&self, other: &Self) -> bool {
        self.seeders == other.seeders && self.leechers == other.leechers
    }
}

impl Eq for Peers {}

impl std::hash::Hash for Peers {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.seeders.hash(state);
        self.leechers.hash(state);
    }
}
