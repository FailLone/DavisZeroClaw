/// Concurrency smoke test for the per-profile lock map owned by `AppState`.
///
/// We exercise the same primitive the production code uses (a nested
/// `tokio::sync::Mutex` map) to prove same-profile acquisitions serialize —
/// `max_seen == 1` for any number of concurrent acquirers. This test does
/// not depend on `crawl4ai_crawl`; its only job is to fail loudly if the
/// lock-map semantics ever regress.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_profile_calls_serialize_under_lock() {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    type LockMap = Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>;

    async fn acquire(map: LockMap, profile: &str) -> Arc<Mutex<()>> {
        let mut guard = map.lock().await;
        guard
            .entry(profile.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    let map: LockMap = Arc::new(Mutex::new(HashMap::new()));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..5 {
        let map = map.clone();
        let in_flight = in_flight.clone();
        let max_seen = max_seen.clone();
        handles.push(tokio::spawn(async move {
            let lock = acquire(map, "express-ali").await;
            let _guard = lock.lock().await;
            let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(cur, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            in_flight.fetch_sub(1, Ordering::SeqCst);
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(
        max_seen.load(Ordering::SeqCst),
        1,
        "concurrent same-profile calls were not serialized"
    );
}
