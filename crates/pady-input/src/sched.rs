//! Keeps gestures instant on a busy computer.
//!
//! * The touchpad, engine and action threads run at the highest normal
//!   priority, so a game or a compile can't make a gesture wait for a CPU.
//!   They sleep almost all the time, so this costs nothing while idle.
//! * Windows timers tick every ~15.6 ms by default, which can make a hold,
//!   double tap or drag release fire up to that much late. While one of
//!   those timers is pending, Pady asks for 1 ms timers and gives them back
//!   straight after, so idle battery use is unchanged.
//!
//! On Linux timers are already precise, and raising priority needs extra
//! privileges, so both are no-ops there.

use pady_core::Scheduling;

/// The hooks for [`pady_core::Engine::spawn_with`].
pub fn scheduling() -> Scheduling {
    Scheduling { thread_start: raise_thread_priority, precise_timers }
}

/// Raises the calling thread above normal apps.
#[cfg(windows)]
pub fn raise_thread_priority() {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
    if let Err(e) = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) } {
        log::warn!("could not raise thread priority: {e}");
    }
}

#[cfg(not(windows))]
pub fn raise_thread_priority() {}

/// `true` asks for 1 ms timers, `false` gives them back. Calls must pair up.
#[cfg(windows)]
pub fn precise_timers(on: bool) {
    use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
    let result = unsafe {
        if on {
            timeBeginPeriod(1)
        } else {
            timeEndPeriod(1)
        }
    };
    if result != 0 {
        log::debug!("timer resolution change failed ({result})");
    }
}

#[cfg(not(windows))]
pub fn precise_timers(_on: bool) {}

#[cfg(all(test, windows))]
mod tests {
    use std::time::{Duration, Instant};
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadPriority, THREAD_PRIORITY_HIGHEST};

    #[test]
    fn raises_the_thread_priority() {
        std::thread::spawn(|| {
            super::raise_thread_priority();
            assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, THREAD_PRIORITY_HIGHEST.0);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn precise_timers_make_short_waits_short() {
        super::precise_timers(true);
        let start = Instant::now();
        for _ in 0..20 {
            let (_tx, rx) = crossbeam_channel::bounded::<()>(0);
            let _ = rx.recv_timeout(Duration::from_millis(1));
        }
        let average = start.elapsed() / 20;
        super::precise_timers(false);
        // Without precise timers each 1 ms wait takes ~15.6 ms.
        assert!(average < Duration::from_millis(6), "average 1 ms wait took {average:?}");
    }
}
