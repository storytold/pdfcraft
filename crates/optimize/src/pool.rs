//! Work over many pages or images on several threads, the results in order, so that what is
//! written is the same however many threads did it.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use crate::OptimizeError;

/// Threads beyond this add more memory than speed: each holds a page or an image being worked on.
const MAX_WORKERS: usize = 8;
/// How long Cancel can wait while every thread is busy with something long.
const CANCEL_CHECK: Duration = Duration::from_millis(100);

/// Let other programs go first on this thread: the wait for a copy matters less than whatever else
/// is open. A system that refuses leaves it as it was.
pub fn give_way_to_other_programs() {
    #[cfg(not(target_arch = "wasm32"))]
    let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
}

/// How many threads to use: `asked`, or half the machine's processors up to [`MAX_WORKERS`], so a
/// bigger machine goes faster and every machine keeps room for other programs. One on the web, which
/// has no threads here.
pub(crate) fn workers(asked: Option<usize>) -> usize {
    if cfg!(target_arch = "wasm32") {
        return 1;
    }
    asked.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).clamp(1, MAX_WORKERS))).max(1)
}

/// `work` on each of `items`, results in the order of `items`. `tick` hears how many are done, and
/// stops the run by returning false. With more than one worker an item whose work panics gets
/// `None`, and the caller decides what becomes of it; with one, the panic goes on up as it does for
/// any other work.
pub(crate) fn map<T: Sync, R: Send>(
    items: &[T],
    workers: usize,
    work: impl Fn(&T) -> R + Sync,
    tick: &mut dyn FnMut(usize) -> bool,
) -> Result<Vec<Option<R>>, OptimizeError> {
    let mut out: Vec<Option<R>> = Vec::with_capacity(items.len());
    if workers <= 1 || items.len() <= 1 {
        for (done, item) in items.iter().enumerate() {
            if !tick(done) {
                return Err(OptimizeError::Cancelled);
            }
            out.push(Some(work(item)));
        }
        return Ok(out);
    }
    out.resize_with(items.len(), || None);
    let (next, stop) = (AtomicUsize::new(0), AtomicBool::new(false));
    let mut stopped = false;
    std::thread::scope(|scope| {
        let (results, finished) = channel();
        for _ in 0..workers.min(items.len()) {
            let results = results.clone();
            let (next, stop, work) = (&next, &stop, &work);
            // A thread that can't start only leaves more for the others, or for below.
            let _ = std::thread::Builder::new().name("pdfcraft-optimize-worker".into()).spawn_scoped(scope, move || {
                give_way_to_other_programs();
                while !stop.load(Ordering::Relaxed) {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let result = catch_unwind(AssertUnwindSafe(|| work(item))).ok();
                    if results.send((i, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(results);
        let mut done = 0;
        loop {
            if !stopped && !tick(done) {
                stopped = true;
                stop.store(true, Ordering::Relaxed);
            }
            match finished.recv_timeout(CANCEL_CHECK) {
                Ok((i, result)) => {
                    if let Some(slot) = out.get_mut(i) {
                        *slot = result;
                    }
                    done += 1;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    if stopped {
        return Err(OptimizeError::Cancelled);
    }
    // No thread could start, or the last of them stopped early: the rest are done here.
    let claimed = next.load(Ordering::Relaxed).min(items.len());
    for (done, (item, slot)) in items.iter().zip(out.iter_mut()).enumerate().skip(claimed) {
        if !tick(done) {
            return Err(OptimizeError::Cancelled);
        }
        *slot = Some(work(item));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn results_keep_their_order_whatever_finishes_first() {
        let items: Vec<u64> = (0..200).collect();
        let out = map(
            &items,
            4,
            |i| {
                // The early ones finish last.
                std::thread::sleep(Duration::from_micros(200u64.saturating_sub(*i)));
                i * 2
            },
            &mut |_| true,
        )
        .unwrap();
        assert_eq!(out, items.iter().map(|i| Some(i * 2)).collect::<Vec<_>>());
    }

    #[test]
    fn one_worker_does_them_in_order_on_this_thread() {
        let here = std::thread::current().id();
        let order = Mutex::new(Vec::new());
        let mut ticks = Vec::new();
        map(
            &[1, 2, 3],
            1,
            |i| {
                assert_eq!(std::thread::current().id(), here);
                order.lock().unwrap().push(*i);
            },
            &mut |done| {
                ticks.push(done);
                true
            },
        )
        .unwrap();
        assert_eq!(*order.lock().unwrap(), [1, 2, 3]);
        assert_eq!(ticks, [0, 1, 2]);
    }

    #[test]
    fn several_workers_share_the_work() {
        let threads = Mutex::new(std::collections::HashSet::new());
        map(
            &[0; 64],
            4,
            |_| {
                threads.lock().unwrap().insert(std::thread::current().id());
                std::thread::sleep(Duration::from_millis(5));
            },
            &mut |_| true,
        )
        .unwrap();
        assert!(threads.lock().unwrap().len() > 1, "it really ran on more than one");
    }

    #[test]
    fn stopping_stops_and_says_so() {
        let ran = AtomicUsize::new(0);
        let r = map(
            &[0; 500],
            4,
            |_| {
                ran.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(1));
            },
            &mut |done| done < 8,
        );
        assert!(matches!(r, Err(OptimizeError::Cancelled)));
        assert!(ran.load(Ordering::Relaxed) < 500, "the rest never started");
    }

    /// Cancel is heard while every thread is busy with something long, not only between items.
    #[test]
    fn a_stop_gets_through_while_every_thread_is_busy() {
        let mut asked_while_busy = 0;
        map(&[0; 2], 2, |_| std::thread::sleep(Duration::from_millis(600)), &mut |done| {
            if done == 0 {
                asked_while_busy += 1;
            }
            true
        })
        .unwrap();
        assert!(asked_while_busy >= 3, "asked {asked_while_busy} times before anything finished");

        let started = std::time::Instant::now();
        let mut asked = 0;
        let r = map(&[0; 4], 2, |_| std::thread::sleep(Duration::from_millis(400)), &mut |_| {
            asked += 1;
            asked < 3
        });
        assert!(matches!(r, Err(OptimizeError::Cancelled)));
        assert!(started.elapsed() < Duration::from_millis(1500), "the items not started never ran");
    }

    #[test]
    fn an_item_that_panics_is_left_alone_and_the_rest_are_done() {
        let items: Vec<u32> = (0..32).collect();
        let out = map(
            &items,
            4,
            |i| {
                assert_ne!(*i, 7, "this one panics");
                *i
            },
            &mut |_| true,
        )
        .unwrap();
        for (i, got) in out.iter().enumerate() {
            assert_eq!(*got, (i != 7).then_some(i as u32), "item {i}");
        }
    }

    /// The workers give way, and the thread that asked keeps its own priority.
    #[cfg(windows)]
    #[test]
    fn workers_give_way_to_other_programs() {
        use thread_priority::get_current_thread_priority;
        let here = get_current_thread_priority().unwrap();
        let theirs = Mutex::new(Vec::new());
        map(
            &[0; 8],
            2,
            |_| {
                theirs.lock().unwrap().push(get_current_thread_priority().unwrap());
                std::thread::sleep(Duration::from_millis(2));
            },
            &mut |_| true,
        )
        .unwrap();
        assert_eq!(get_current_thread_priority().unwrap(), here);
        let theirs = theirs.into_inner().unwrap();
        assert_eq!(theirs.len(), 8);
        assert!(theirs.iter().all(|p| *p != here), "{theirs:?} against {here:?}");
    }

    #[test]
    fn nothing_to_do_is_fine() {
        let none: [u8; 0] = [];
        assert!(map(&none, 4, |_| 1, &mut |_| true).unwrap().is_empty());
        assert_eq!(map(&[5], 4, |i| *i, &mut |_| true).unwrap(), [Some(5)]);
    }

    #[test]
    fn the_web_and_small_machines_get_a_sensible_count() {
        assert_eq!(workers(Some(3)), if cfg!(target_arch = "wasm32") { 1 } else { 3 });
        assert_eq!(workers(Some(0)), 1, "never none");
        let picked = workers(None);
        assert!((1..=MAX_WORKERS).contains(&picked), "{picked}");
    }
}
