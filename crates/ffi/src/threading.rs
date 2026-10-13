//! Threading policy, worker start hook, thread QoS (Apple), and priority (Android).

use std::sync::atomic::{AtomicBool, Ordering};

static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

/// Configures the render thread start hook for mobile worker threads.
/// On Apple platforms, sets thread QoS class to user-initiated.
/// On Android, sets thread priority to default process priority.
#[allow(unsafe_code)]
#[uniffi::export]
pub fn init_worker_threading_hook() {
    if HOOK_INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }

    pdfcraft_render::set_worker_start_hook(|| {
        #[cfg(target_vendor = "apple")]
        {
            // SAFETY: pthread_set_qos_class_self_np operates strictly on the calling thread
            // with well-defined Apple POSIX constants and produces no memory corruption.
            unsafe {
                libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INITIATED, 0);
            }
        }

        #[cfg(target_os = "android")]
        {
            // SAFETY: setpriority operates on the current calling thread (who = 0).
            // Setting nice value 0 ensures responsive rendering without stalling compositor.
            unsafe {
                libc::setpriority(libc::PRIO_PROCESS, 0, 0);
            }
        }
    });
}

/// Returns the recommended worker thread count capped to performance cores.
#[uniffi::export]
pub fn recommended_worker_threads() -> u32 {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    // On big.LITTLE mobile architectures, performance cores are roughly half the total cores.
    // We cap at 4 to prevent thermal throttling and battery drain during sustained scrolling.
    ((cores / 2).max(1) as u32).min(4)
}
