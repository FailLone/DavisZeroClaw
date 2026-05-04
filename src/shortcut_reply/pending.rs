//! Stub — filled in by the pending task.
//! Transitional: scaffolded in Task 2; real impl lands in Task 5. Remove
//! this attribute when the real file content replaces the stubs.
#![allow(dead_code)]
pub struct PendingReplies;
pub enum TakeResult {
    Unknown,
}
pub fn spawn_gc_task(
    _: std::sync::Arc<PendingReplies>,
    _max_age: std::time::Duration,
    _interval: std::time::Duration,
) {
}
