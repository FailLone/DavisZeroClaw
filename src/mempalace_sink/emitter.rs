//! Decoupling trait so subsystem hooks don't depend on the concrete
//! `MemPalaceSink`. Makes Phase 2+ hooks testable with a `SpySink` that
//! records events in memory.

use super::driver::MemPalaceSink;
use super::predicate::{Predicate, TripleId};

/// Narrow fire-and-forget surface the subsystem hooks depend on.
pub trait MempalaceEmitter: Send + Sync {
    fn add_drawer(&self, wing: &str, room: &str, content: &str);
    fn kg_add(&self, subject: TripleId, predicate: Predicate, object: TripleId);
    fn kg_invalidate(&self, subject: TripleId, predicate: Predicate, object: TripleId);
    fn diary_write(&self, wing: &str, entry: &str);
}

impl MempalaceEmitter for MemPalaceSink {
    fn add_drawer(&self, wing: &str, room: &str, content: &str) {
        MemPalaceSink::add_drawer(self, wing, room, content);
    }
    fn kg_add(&self, subject: TripleId, predicate: Predicate, object: TripleId) {
        MemPalaceSink::kg_add(self, subject, predicate, object);
    }
    fn kg_invalidate(&self, subject: TripleId, predicate: Predicate, object: TripleId) {
        MemPalaceSink::kg_invalidate(self, subject, predicate, object);
    }
    fn diary_write(&self, wing: &str, entry: &str) {
        MemPalaceSink::diary_write(self, wing, entry);
    }
}

#[cfg(test)]
pub struct SpySink {
    inner: std::sync::Mutex<SpyState>,
}

#[cfg(test)]
#[derive(Default)]
struct SpyState {
    drawers: Vec<SpyDrawer>,
    kg_adds: Vec<SpyTriple>,
    kg_invalidates: Vec<SpyTriple>,
    diary_entries: Vec<SpyDiary>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpyDrawer {
    pub wing: String,
    pub room: String,
    pub content: String,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpyTriple {
    pub subject: String,
    pub predicate: Predicate,
    pub object: String,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpyDiary {
    pub wing: String,
    pub entry: String,
}

#[cfg(test)]
impl Default for SpySink {
    fn default() -> Self {
        Self {
            inner: std::sync::Mutex::new(SpyState::default()),
        }
    }
}

#[cfg(test)]
impl SpySink {
    pub fn drawers(&self) -> Vec<SpyDrawer> {
        self.inner.lock().unwrap().drawers.clone()
    }
    pub fn kg_adds(&self) -> Vec<SpyTriple> {
        self.inner.lock().unwrap().kg_adds.clone()
    }
    pub fn kg_invalidates(&self) -> Vec<SpyTriple> {
        self.inner.lock().unwrap().kg_invalidates.clone()
    }
    pub fn diary_entries(&self) -> Vec<SpyDiary> {
        self.inner.lock().unwrap().diary_entries.clone()
    }
}

#[cfg(test)]
impl MempalaceEmitter for SpySink {
    fn add_drawer(&self, wing: &str, room: &str, content: &str) {
        self.inner.lock().unwrap().drawers.push(SpyDrawer {
            wing: wing.to_string(),
            room: room.to_string(),
            content: content.to_string(),
        });
    }
    fn kg_add(&self, subject: TripleId, predicate: Predicate, object: TripleId) {
        self.inner.lock().unwrap().kg_adds.push(SpyTriple {
            subject: subject.as_str().to_string(),
            predicate,
            object: object.as_str().to_string(),
        });
    }
    fn kg_invalidate(&self, subject: TripleId, predicate: Predicate, object: TripleId) {
        self.inner.lock().unwrap().kg_invalidates.push(SpyTriple {
            subject: subject.as_str().to_string(),
            predicate,
            object: object.as_str().to_string(),
        });
    }
    fn diary_write(&self, wing: &str, entry: &str) {
        self.inner.lock().unwrap().diary_entries.push(SpyDiary {
            wing: wing.to_string(),
            entry: entry.to_string(),
        });
    }
}
