//! Per-torrent seeding statistics reported back to the tracker.
//!
//! Port of Java `org.araymond.joal.core.bandwith.TorrentSeedStats`. The Java
//! version exposes `getUploaded()` / `getDownloaded()` / `getLeft()` and a
//! package-private `addUploaded(long)`. Rust keeps the same surface plus an
//! internal setter-pair for `downloaded`/`left` (which today always stay at
//! 0, but are here for when JOAL grows real download state).

/// Running torrent statistics announced to the tracker.
///
/// `uploaded` is the only counter that moves today. `downloaded` and `left`
/// are reserved for future use and always report `0`, matching Java.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TorrentSeedStats {
    uploaded: u64,
    downloaded: u64,
    left: u64,
}

impl TorrentSeedStats {
    #[must_use]
    pub const fn new(uploaded: u64, downloaded: u64, left: u64) -> Self {
        Self {
            uploaded,
            downloaded,
            left,
        }
    }

    #[must_use]
    pub const fn uploaded(&self) -> u64 {
        self.uploaded
    }

    #[must_use]
    pub const fn downloaded(&self) -> u64 {
        self.downloaded
    }

    #[must_use]
    pub const fn left(&self) -> u64 {
        self.left
    }

    /// Java `addUploaded(long)` — saturating to keep the counter monotonic
    /// across very long seeding sessions.
    pub fn add_uploaded(&mut self, bytes: u64) {
        self.uploaded = self.uploaded.saturating_add(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_zero() {
        let s = TorrentSeedStats::default();
        assert_eq!(s.uploaded(), 0);
        assert_eq!(s.downloaded(), 0);
        assert_eq!(s.left(), 0);
    }

    #[test]
    fn add_uploaded_accumulates() {
        let mut s = TorrentSeedStats::default();
        s.add_uploaded(50);
        assert_eq!(s.uploaded(), 50);
        s.add_uploaded(75);
        assert_eq!(s.uploaded(), 125);
    }

    #[test]
    fn add_uploaded_saturates_on_overflow() {
        let mut s = TorrentSeedStats::new(u64::MAX - 5, 0, 0);
        s.add_uploaded(100);
        assert_eq!(s.uploaded(), u64::MAX);
    }
}
