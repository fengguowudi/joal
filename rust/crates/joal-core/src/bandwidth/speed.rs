//! Current upload speed assigned to a single torrent.
//!
//! Port of Java `org.araymond.joal.core.bandwith.Speed`. The Java class is a
//! mutable POJO with a single `long bytesPerSecond` field; Rust uses an
//! interior-mut wrapper intentionally — [`BandwidthDispatcher`][super::BandwidthDispatcher]
//! mutates it via [`Speed::set_bytes_per_second`] while announcers read it
//! through [`Speed::bytes_per_second`].

/// Instantaneous upload allocation for one torrent, in bytes per second.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Speed {
    bytes_per_second: u64,
}

impl Speed {
    #[must_use]
    pub const fn new(bytes_per_second: u64) -> Self {
        Self { bytes_per_second }
    }

    #[must_use]
    pub const fn bytes_per_second(&self) -> u64 {
        self.bytes_per_second
    }

    pub fn set_bytes_per_second(&mut self, bytes_per_second: u64) {
        self.bytes_per_second = bytes_per_second;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_stores_rate() {
        let s = Speed::new(1_024);
        assert_eq!(s.bytes_per_second(), 1_024);
    }

    #[test]
    fn set_mutates_rate() {
        let mut s = Speed::default();
        s.set_bytes_per_second(2_048);
        assert_eq!(s.bytes_per_second(), 2_048);
    }
}
