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

#[cfg(test)]
mod tests {
    use super::*;

    fn ih(x: u8) -> InfoHash {
        let mut bytes = [0u8; 20];
        bytes[0] = x;
        InfoHash::from_bytes(bytes)
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Item {
        info_hash: InfoHash,
        tag: u32,
    }

    impl InfoHashAble for Item {
        fn info_hash(&self) -> &InfoHash {
            &self.info_hash
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn dedup_by_info_hash_keeps_only_last_insert() {
        let q = DelayQueue::<Item>::new();
        q.add_or_replace(
            Item {
                info_hash: ih(1),
                tag: 1,
            },
            Duration::from_secs(10),
        );
        q.add_or_replace(
            Item {
                info_hash: ih(1),
                tag: 2,
            },
            Duration::from_secs(10),
        );
        assert_eq!(q.len(), 1);

        tokio::time::advance(Duration::from_secs(11)).await;
        let got = q.get_availables();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].tag, 2);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn get_availables_respects_release_order() {
        let q = DelayQueue::<Item>::new();
        q.add_or_replace(
            Item {
                info_hash: ih(1),
                tag: 10,
            },
            Duration::from_secs(5),
        );
        q.add_or_replace(
            Item {
                info_hash: ih(2),
                tag: 20,
            },
            Duration::from_secs(1),
        );
        q.add_or_replace(
            Item {
                info_hash: ih(3),
                tag: 30,
            },
            Duration::from_secs(3),
        );

        tokio::time::advance(Duration::from_secs(2)).await;
        let got = q.get_availables();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].tag, 20);

        tokio::time::advance(Duration::from_secs(4)).await;
        let got = q.get_availables();
        let tags: Vec<_> = got.iter().map(|i| i.tag).collect();
        assert_eq!(tags, vec![30, 10]);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn same_millisecond_insertions_preserve_order() {
        let q = DelayQueue::<Item>::new();
        q.add_or_replace(
            Item {
                info_hash: ih(1),
                tag: 1,
            },
            Duration::ZERO,
        );
        q.add_or_replace(
            Item {
                info_hash: ih(2),
                tag: 2,
            },
            Duration::ZERO,
        );
        q.add_or_replace(
            Item {
                info_hash: ih(3),
                tag: 3,
            },
            Duration::ZERO,
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        let got = q.get_availables();
        let tags: Vec<_> = got.iter().map(|i| i.tag).collect();
        assert_eq!(tags, vec![1, 2, 3]);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn drain_all_returns_every_entry_regardless_of_release() {
        let q = DelayQueue::<Item>::new();
        q.add_or_replace(
            Item {
                info_hash: ih(1),
                tag: 1,
            },
            Duration::from_secs(100),
        );
        q.add_or_replace(
            Item {
                info_hash: ih(2),
                tag: 2,
            },
            Duration::from_secs(200),
        );
        let drained = q.drain_all();
        assert_eq!(drained.len(), 2);
        assert!(q.is_empty());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn remove_by_info_hash_drops_matching_entry() {
        let q = DelayQueue::<Item>::new();
        q.add_or_replace(
            Item {
                info_hash: ih(1),
                tag: 1,
            },
            Duration::from_secs(10),
        );
        q.add_or_replace(
            Item {
                info_hash: ih(2),
                tag: 2,
            },
            Duration::from_secs(10),
        );
        q.remove(&ih(1));
        assert_eq!(q.len(), 1);
        tokio::time::advance(Duration::from_secs(11)).await;
        let got = q.get_availables();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].tag, 2);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn head_not_due_returns_empty() {
        let q = DelayQueue::<Item>::new();
        q.add_or_replace(
            Item {
                info_hash: ih(1),
                tag: 1,
            },
            Duration::from_mins(1),
        );
        assert!(q.get_availables().is_empty());
    }
}
