//! Thread spawning that reports OS failures instead of panicking.
//!
//! `std::thread::spawn` panics when the OS refuses a new thread (for example
//! EAGAIN at a process or cgroup task limit). In the server that panic either
//! exits the whole process or silently kills a long-lived loop, so server code
//! spawns through these helpers and degrades per call site.

use std::io;
use std::thread::JoinHandle;

pub(crate) fn spawn_named<F, T>(name: &str, f: F) -> io::Result<JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    #[cfg(test)]
    if test_hook::take_injected_failure() {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "injected thread spawn failure",
        ));
    }
    std::thread::Builder::new().name(name.to_owned()).spawn(f)
}

#[cfg(test)]
pub(crate) mod test_hook {
    use std::cell::Cell;

    thread_local! {
        static INJECTED_FAILURES: Cell<usize> = const { Cell::new(0) };
        static ALLOWED_BEFORE_FAILURES: Cell<usize> = const { Cell::new(0) };
    }

    /// Makes the next `count` spawns from the current thread fail.
    pub(crate) fn fail_next_spawns(count: usize) {
        fail_spawns_after(0, count);
    }

    /// Lets `allowed` spawns from the current thread succeed, then fails `count`.
    pub(crate) fn fail_spawns_after(allowed: usize, count: usize) {
        ALLOWED_BEFORE_FAILURES.with(|cell| cell.set(allowed));
        INJECTED_FAILURES.with(|failures| failures.set(count));
    }

    pub(super) fn take_injected_failure() -> bool {
        let remaining = INJECTED_FAILURES.with(Cell::get);
        if remaining == 0 {
            return false;
        }
        let allowed = ALLOWED_BEFORE_FAILURES.with(Cell::get);
        if allowed > 0 {
            ALLOWED_BEFORE_FAILURES.with(|cell| cell.set(allowed - 1));
            return false;
        }
        INJECTED_FAILURES.with(|failures| failures.set(remaining - 1));
        true
    }
}
