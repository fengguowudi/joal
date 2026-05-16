//! Per-torrent seeding statistics reported back to the tracker.
//!
//! Port of Java `org.araymond.joal.core.bandwith.TorrentSeedStats`. The Java
//! version exposes `getUploaded()` / `getDownloaded()` / `getLeft()` and a
//! package-private `addUploaded(long)`. The Rust port keeps the same surface
//! and additionally lets the bandwidth dispatcher *write* `downloaded` /
//! `left` so the download-faker (Rust-only feature, see
//! [`AppConfiguration::min_download_rate`][crate::config::AppConfiguration]) can
//! tally simulated download progress.

/// Result of [`TorrentSeedStats::add_downloaded`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadEdge {
    /// Increment applied; torrent is still downloading.
    InProgress,
    /// `downloaded` reached `total_size` *during this call*.
    /// Caller is expected to fire an `event=completed` announce next.
    JustCompleted,
    /// Already at `total_size` before this call; nothing was added.
    AlreadyCompleted,
}

/// Running torrent statistics announced to the tracker.
///
/// `uploaded` accumulates from the upload faker; `downloaded` / `left`
/// accumulate from the download faker when configured (otherwise they stay
/// at the initial values, matching Java's "always-zero" behaviour).
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

    /// Initial state for a torrent that starts as a leecher (downloaded=0,
    /// left=total_size). Convenience for the dispatcher.
    #[must_use]
    pub const fn fresh(total_size: u64) -> Self {
        Self::new(0, 0, total_size)
    }

    /// Initial state for a torrent the user marked "initial completed" —
    /// downloaded=total_size, left=0. First announce will report a finished
    /// download.
    #[must_use]
    pub const fn completed(total_size: u64) -> Self {
        Self::new(0, total_size, 0)
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

    #[must_use]
    pub const fn is_completed(&self) -> bool {
        self.left == 0 && self.downloaded > 0
    }

    /// Java `addUploaded(long)` — saturating to keep the counter monotonic
    /// across very long seeding sessions.
    pub fn add_uploaded(&mut self, bytes: u64) {
        self.uploaded = self.uploaded.saturating_add(bytes);
    }

    /// Credit `bytes` to `downloaded`, capped at `total_size`. Updates
    /// `left = total_size - downloaded` in lock-step. Returns a
    /// [`DownloadEdge`] so the caller can detect the leecher → seeder
    /// transition (which triggers an `event=completed` announce).
    pub fn add_downloaded(&mut self, bytes: u64, total_size: u64) -> DownloadEdge {
        if total_size == 0 {
            // Zero-size torrent: nothing to download, treat as already done.
            self.downloaded = 0;
            self.left = 0;
            return DownloadEdge::AlreadyCompleted;
        }
        if self.downloaded >= total_size {
            // Already at cap — keep `left` truthful and bail.
            self.downloaded = total_size;
            self.left = 0;
            return DownloadEdge::AlreadyCompleted;
        }
        let new_total = self.downloaded.saturating_add(bytes).min(total_size);
        self.downloaded = new_total;
        self.left = total_size - new_total;
        if self.downloaded == total_size {
            DownloadEdge::JustCompleted
        } else {
            DownloadEdge::InProgress
        }
    }

    /// Direct setter used when a torrent is force-marked "initial completed"
    /// at runtime (UI checkbox). Keeps `left` consistent.
    pub fn force_completed(&mut self, total_size: u64) {
        self.downloaded = total_size;
        self.left = 0;
    }

    /// Direct setter used when "initial completed" is unchecked at runtime —
    /// resets the download progress to zero so the next regular announce
    /// reports `downloaded=0&left=total_size`.
    pub fn reset_download(&mut self, total_size: u64) {
        self.downloaded = 0;
        self.left = total_size;
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

    #[test]
    fn fresh_starts_as_full_leecher() {
        let s = TorrentSeedStats::fresh(1_000);
        assert_eq!(s.uploaded(), 0);
        assert_eq!(s.downloaded(), 0);
        assert_eq!(s.left(), 1_000);
        assert!(!s.is_completed());
    }

    #[test]
    fn completed_starts_as_full_seeder() {
        let s = TorrentSeedStats::completed(1_000);
        assert_eq!(s.downloaded(), 1_000);
        assert_eq!(s.left(), 0);
        assert!(s.is_completed());
    }

    #[test]
    fn add_downloaded_progresses_and_caps() {
        let mut s = TorrentSeedStats::fresh(1_000);
        assert_eq!(s.add_downloaded(300, 1_000), DownloadEdge::InProgress);
        assert_eq!(s.downloaded(), 300);
        assert_eq!(s.left(), 700);

        // Cap at total_size on overshoot.
        assert_eq!(s.add_downloaded(900, 1_000), DownloadEdge::JustCompleted);
        assert_eq!(s.downloaded(), 1_000);
        assert_eq!(s.left(), 0);

        // Subsequent calls are idempotent and reported as AlreadyCompleted.
        assert_eq!(s.add_downloaded(50, 1_000), DownloadEdge::AlreadyCompleted);
        assert_eq!(s.downloaded(), 1_000);
        assert_eq!(s.left(), 0);
    }

    #[test]
    fn add_downloaded_zero_total_is_already_completed() {
        let mut s = TorrentSeedStats::fresh(0);
        assert_eq!(s.add_downloaded(100, 0), DownloadEdge::AlreadyCompleted);
        assert_eq!(s.downloaded(), 0);
        assert_eq!(s.left(), 0);
    }

    #[test]
    fn force_completed_and_reset_keep_left_consistent() {
        let mut s = TorrentSeedStats::fresh(1_000);
        s.force_completed(1_000);
        assert_eq!(s.downloaded(), 1_000);
        assert_eq!(s.left(), 0);
        s.reset_download(1_000);
        assert_eq!(s.downloaded(), 0);
        assert_eq!(s.left(), 1_000);
    }
}
