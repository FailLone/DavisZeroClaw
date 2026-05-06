//! Owns the `pending_replies` table. Every insertion, lookup, abandon,
//! and GC pass goes through this module's methods; nothing else touches
//! the inner map.

use crate::shortcut_reply::types::{PendingReply, RequestId, ShortcutResponse};
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

const RECENTLY_DELIVERED_CAPACITY: usize = 64;
const RECENTLY_DELIVERED_TTL: Duration = Duration::from_secs(30);

pub struct PendingReplies {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    waiting: HashMap<RequestId, PendingReply>,
    recently_delivered: LruCache<RequestId, Instant>,
}

pub enum TakeResult {
    Found(PendingReply),
    AlreadyDelivered,
    Unknown,
}

impl Default for PendingReplies {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingReplies {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                waiting: HashMap::new(),
                recently_delivered: LruCache::new(
                    NonZeroUsize::new(RECENTLY_DELIVERED_CAPACITY).expect("capacity > 0"),
                ),
            })),
        }
    }

    /// Register a new pending reply. Returns the generated `request_id`
    /// (uuid v4 string) and the `oneshot::Receiver` the caller should
    /// await.
    pub fn register(
        &self,
        imessage_handle: Option<String>,
    ) -> (RequestId, oneshot::Receiver<ShortcutResponse>) {
        let (tx, rx) = oneshot::channel();
        let request_id = uuid::Uuid::new_v4().to_string();
        let entry = PendingReply {
            request_id: request_id.clone(),
            sender: tx,
            created_at: Instant::now(),
            imessage_handle,
            imessage_sent: false,
            abandoned: false,
        };
        let mut inner = self.inner.lock().expect("pending_replies lock poisoned");
        inner.waiting.insert(request_id.clone(), entry);
        (request_id, rx)
    }

    /// Mark the entry as abandoned. Called by the Shortcut bridge
    /// handler when its oneshot timeout fires.
    ///
    /// Returns `Some(())` if the entry was still waiting (we leave it
    /// in the map so the reply handler can still find it and run the
    /// iMessage fallback). Returns `None` if the entry was already
    /// taken by the reply handler.
    pub fn abandon(&self, id: &RequestId) -> Option<()> {
        let mut inner = self.inner.lock().expect("pending_replies lock poisoned");
        if let Some(entry) = inner.waiting.get_mut(id) {
            entry.abandoned = true;
            Some(())
        } else {
            None
        }
    }

    /// Remove the entry and return it. If not present, check
    /// `recently_delivered` to decide between `AlreadyDelivered` and
    /// `Unknown`. Also prunes expired LRU entries lazily.
    ///
    /// Note: the `recently_delivered` LRU is bounded by
    /// `RECENTLY_DELIVERED_CAPACITY` (64). After that many successful
    /// `take()` calls, the oldest entry is evicted silently. A
    /// subsequent re-delivery of that evicted id returns `Unknown`
    /// rather than `AlreadyDelivered` — callers (the relay handler)
    /// must treat both equivalently (log + ignore) to stay robust
    /// against this narrow idempotency gap.
    pub fn take(&self, id: &RequestId) -> TakeResult {
        let mut inner = self.inner.lock().expect("pending_replies lock poisoned");
        if let Some(entry) = inner.waiting.remove(id) {
            inner.recently_delivered.put(id.clone(), Instant::now());
            return TakeResult::Found(entry);
        }
        // Expire stale LRU entries on cache-miss reads. Cheap scan — cache is ≤ 64.
        let now = Instant::now();
        let stale: Vec<RequestId> = inner
            .recently_delivered
            .iter()
            .filter_map(|(k, &t)| {
                if now.duration_since(t) > RECENTLY_DELIVERED_TTL {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .collect();
        for k in stale {
            inner.recently_delivered.pop(&k);
        }
        if inner.recently_delivered.contains(id) {
            TakeResult::AlreadyDelivered
        } else {
            TakeResult::Unknown
        }
    }

    /// Purge waiting entries older than `max_age`. Runs under the GC
    /// task. Returns the number of entries purged (for metrics).
    pub fn gc(&self, max_age: Duration) -> usize {
        let now = Instant::now();
        // Two-pass: collect ids under one lock, drop, then remove under a
        // fresh lock. The `is_some()` guard on the second pass silently
        // skips any id a concurrent `take()` already drained between the
        // snapshot and the removal, keeping `purged` accurate.
        let stale: Vec<RequestId> = {
            let inner = self.inner.lock().expect("pending_replies lock poisoned");
            inner
                .waiting
                .iter()
                .filter_map(|(id, entry)| {
                    if now.duration_since(entry.created_at) > max_age {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };
        if stale.is_empty() {
            return 0;
        }
        let mut inner = self.inner.lock().expect("pending_replies lock poisoned");
        let mut purged = 0;
        for id in &stale {
            if inner.waiting.remove(id).is_some() {
                purged += 1;
            }
        }
        purged
    }

    pub fn pending_count(&self) -> usize {
        self.inner
            .lock()
            .expect("pending_replies lock poisoned")
            .waiting
            .len()
    }

    pub fn recently_delivered_count(&self) -> usize {
        self.inner
            .lock()
            .expect("pending_replies lock poisoned")
            .recently_delivered
            .len()
    }
}

/// Spawn a background task that calls `gc(max_age)` every `interval`.
/// The task owns a clone of the `Arc<PendingReplies>` and exits only
/// when all other references to the `Arc` drop.
// called by local_proxy.rs in Task 8
#[allow(dead_code)]
pub fn spawn_gc_task(pending: Arc<PendingReplies>, max_age: Duration, interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let purged = pending.gc(max_age);
            if purged > 0 {
                tracing::info!(
                    target: "shortcut_reply",
                    event = "gc_swept",
                    purged,
                    "pending_replies GC purged stale entries",
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make() -> Arc<PendingReplies> {
        Arc::new(PendingReplies::new())
    }

    #[tokio::test]
    async fn register_returns_unique_ids() {
        let p = make();
        let mut ids = std::collections::HashSet::new();
        for _ in 0..1000 {
            let (id, _rx) = p.register(None);
            assert!(ids.insert(id), "uuid collision");
        }
    }

    #[tokio::test]
    async fn take_returns_found_then_already_delivered() {
        let p = make();
        let (id, _rx) = p.register(None);
        match p.take(&id) {
            TakeResult::Found(_) => {}
            _ => panic!("first take must be Found"),
        }
        match p.take(&id) {
            TakeResult::AlreadyDelivered => {}
            _ => panic!("second take must be AlreadyDelivered"),
        }
    }

    #[tokio::test]
    async fn take_unknown_returns_unknown() {
        let p = make();
        match p.take(&"never-existed".to_string()) {
            TakeResult::Unknown => {}
            _ => panic!("must be Unknown"),
        }
    }

    #[tokio::test]
    async fn abandon_before_take_leaves_entry_accessible() {
        let p = make();
        let (id, _rx) = p.register(None);
        assert!(p.abandon(&id).is_some());
        match p.take(&id) {
            TakeResult::Found(entry) => assert!(entry.abandoned, "abandoned flag must persist"),
            _ => panic!("abandon must not remove entry"),
        }
    }

    #[tokio::test]
    async fn abandon_after_take_returns_none() {
        let p = make();
        let (id, _rx) = p.register(None);
        let _ = p.take(&id);
        assert!(p.abandon(&id).is_none());
    }

    #[tokio::test]
    async fn gc_sweeps_stale_keeps_fresh() {
        let p = make();
        let (old_id, _rx1) = p.register(None);
        // Backdate the old entry directly for the test — the only way
        // without sleeping.
        {
            let mut inner = p.inner.lock().unwrap();
            inner.waiting.get_mut(&old_id).unwrap().created_at =
                Instant::now() - Duration::from_secs(3600);
        }
        let (fresh_id, _rx2) = p.register(None);
        let purged = p.gc(Duration::from_secs(300));
        assert_eq!(purged, 1);
        assert!(p.inner.lock().unwrap().waiting.contains_key(&fresh_id));
        assert!(!p.inner.lock().unwrap().waiting.contains_key(&old_id));
    }

    #[tokio::test]
    async fn concurrent_register_take_100_tasks_no_panic() {
        let p = make();
        let mut handles = Vec::new();
        for _ in 0..100 {
            let p = p.clone();
            handles.push(tokio::spawn(async move {
                let (id, _rx) = p.register(None);
                let _ = p.take(&id);
            }));
        }
        for h in handles {
            h.await.expect("task");
        }
        assert_eq!(p.pending_count(), 0);
    }

    #[tokio::test]
    async fn lru_eviction_beyond_capacity_returns_unknown() {
        let p = make();
        // Fill the LRU past capacity. The first take() to land in the
        // cache is the one that gets evicted when the 65th insertion
        // (capacity + 1) arrives.
        let mut first_id = String::new();
        for i in 0..=RECENTLY_DELIVERED_CAPACITY {
            let (id, _rx) = p.register(None);
            if i == 0 {
                first_id = id.clone();
            }
            let _ = p.take(&id);
        }
        assert_eq!(p.recently_delivered_count(), RECENTLY_DELIVERED_CAPACITY);
        match p.take(&first_id) {
            TakeResult::Unknown => {}
            TakeResult::AlreadyDelivered => {
                panic!("first id should have been evicted by capacity limit")
            }
            TakeResult::Found(_) => panic!("first id was already taken above"),
        }
    }
}
