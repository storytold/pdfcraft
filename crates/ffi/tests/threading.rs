use pdfcraft_ffi::{init_worker_threading_hook, recommended_worker_threads};

#[test]
fn test_recommended_worker_threads_is_bounded() {
    let threads = recommended_worker_threads();
    assert!(threads >= 1, "must have at least 1 worker thread");
    assert!(threads <= 4, "must be capped to performance cores (at most 4) to prevent thermal throttling");
}

#[test]
fn test_worker_threading_hook_idempotent() {
    init_worker_threading_hook();
    // Subsequent calls must be idempotent and panic-free
    init_worker_threading_hook();
}
