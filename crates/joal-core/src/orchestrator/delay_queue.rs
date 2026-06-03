//! Priority queue keyed on `InfoHash` with release-time based scheduling.
//!
//! Port of Java `org.araymond.joal.core.ttorrent.client.DelayQueue`. The Rust
//! implementation keeps the same observable semantics:
//!
//! - `add_or_replace(item, delay)` dedups by `InfoHash`: adding a new
//!   announce request for a torrent replaces any existing queued request for
//!   that torrent. Matches Java's `removeIf(infoHashEquals(item))`.
//! - `get_availables()` drains every entry whose release time has passed, in
//!   release-time order. Matches Java's do/while loop.
//! - `drain_all()` removes everything regardless of release time.
//! - `remove(&InfoHash)` removes by info-hash.
//!
//! # Clock
//!
//! Java uses `LocalDateTime.now()`. The Rust version uses
//! [`tokio::time::Instant`] (a monotonic clock) — better suited to scheduling
//! and safe under `tokio::time::pause`/`advance` in tests.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

use crate::torrent::InfoHash;

/// Types that have an associated [`InfoHash`] — used for dedup keys.
/// Mirrors Java inner interface `DelayQueue.InfoHashAble`.
pub trait InfoHashAble {
    fn info_hash(&self) -> &InfoHash;
}

/// Thread-safe delay queue.
pub struct DelayQueue<T: InfoHashAble + Clone + Send + 'static> {
    inner: Mutex<Inner<T>>,
}

struct Inner<T> {
    // VecDeque + linear search is fine here: the engine queue is bounded by
    // the `simultaneousSeed` cap (usually a single-digit number). Upgrading
    // to a proper heap is easy if that assumption ever breaks.
    items: VecDeque<IntervalAware<T>>,
}

#[derive(Debug, Clone)]
struct IntervalAware<T> {
    item: T,
    release_at: Instant,
    // Insertion order tiebreaker; Java relies on `PriorityQueue`'s unspecified
    // tiebreak, but a deterministic rule here keeps tests reliable on
    // same-millisecond inserts.
    seq: u64,
}

impl<T: InfoHashAble + Clone + Send + 'static> DelayQueue<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                items: VecDeque::new(),
            }),
        }
    }

    /// Insert `item` (or replace the existing entry for the same info-hash)
    /// with the given delay from now.
    pub fn add_or_replace(&self, item: T, delay: Duration) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let seq = inner
            .items
            .back()
            .map_or(0, |last| last.seq.wrapping_add(1));
        inner
            .items
            .retain(|existing| existing.item.info_hash() != item.info_hash());
        let release_at = Instant::now() + delay;
        let new = IntervalAware {
            item,
            release_at,
            seq,
        };
        // Keep the queue sorted by `release_at` then insertion order.
        let pos = inner
            .items
            .iter()
            .position(|existing| (existing.release_at, existing.seq) > (new.release_at, new.seq))
            .unwrap_or(inner.items.len());
        inner.items.insert(pos, new);
    }

    /// Drain and return every entry whose release time has passed. The Java
    /// version returns an empty list if the head is not yet due. We do the
    /// same.
    pub fn get_availables(&self) -> Vec<T> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let mut out = Vec::new();
        while let Some(front) = inner.items.front() {
            if front.release_at > now {
                break;
            }
            let entry = inner
                .items
                .pop_front()
                .expect("front peek succeeded, pop must succeed");
            out.push(entry.item);
        }
        out
    }

    /// Remove and return every queued entry regardless of release time.
    pub fn drain_all(&self) -> Vec<T> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.items.drain(..).map(|e| e.item).collect()
    }

    /// Remove any entry whose info-hash matches `info_hash`.
    pub fn remove(&self, info_hash: &InfoHash) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.items.retain(|e| e.item.info_hash() != info_hash);
    }

    /// Number of queued entries. Primarily for tests and metrics.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .items
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T: InfoHashAble + Clone + Send + 'static> Default for DelayQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: InfoHashAble + Clone + Send + 'static> std::fmt::Debug for DelayQueue<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelayQueue")
            .field("len", &self.len())
            .finish()
    }
}

impl InfoHashAble for crate::announcer::AnnounceRequest {
    fn info_hash(&self) -> &InfoHash {
        crate::announcer::AnnounceRequest::info_hash(self)
    }
}
