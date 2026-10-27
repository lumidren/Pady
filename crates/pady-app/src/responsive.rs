//! Keeps the first gesture after a long idle stretch as quick as any other.
//!
//! Windows treats a quiet tray app as background work: it moves it to
//! efficiency mode (slow cores, coalesced timers) and pages its memory out,
//! so the next touch first has to wake all of that up. Pady opts out of both
//! and runs slightly above normal priority. The cost is a little RAM that
//! stays resident; idle CPU is still zero.

/// Most of Pady's memory that Windows may not page out while it's idle.
#[cfg(windows)]
const RESIDENT_BYTES: usize = 48 << 20;

#[cfg(windows)]
pub fn keep_responsive() {
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows::Win32::System::Memory::{SetProcessWorkingSetSizeEx, QUOTA_LIMITS_HARDWS_MIN_ENABLE};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
        ABOVE_NORMAL_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        PROCESS_POWER_THROTTLING_STATE,
    };

    unsafe {
        let process = GetCurrentProcess();
        // Control both throttles, and set neither: never efficiency mode.
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
            StateMask: 0,
        };
        let info = &state as *const PROCESS_POWER_THROTTLING_STATE as *const c_void;
        if let Err(e) = SetProcessInformation(
            process,
            ProcessPowerThrottling,
            info,
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        ) {
            log::warn!("could not opt out of efficiency mode: {e}");
        }
        if let Err(e) = SetPriorityClass(process, ABOVE_NORMAL_PRIORITY_CLASS) {
            log::warn!("could not raise priority: {e}");
        }
        if let Err(e) =
            SetProcessWorkingSetSizeEx(process, RESIDENT_BYTES, 4 * RESIDENT_BYTES, QUOTA_LIMITS_HARDWS_MIN_ENABLE)
        {
            log::warn!("could not keep memory resident: {e}");
        }
    }
}

#[cfg(not(windows))]
pub fn keep_responsive() {}

#[cfg(all(test, windows))]
mod tests {
    use windows::Win32::System::Memory::{GetProcessWorkingSetSizeEx, QUOTA_LIMITS_HARDWS_MIN_ENABLE};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetPriorityClass, ABOVE_NORMAL_PRIORITY_CLASS};

    #[test]
    fn opts_out_of_idle_slowdowns() {
        super::keep_responsive();
        unsafe {
            let process = GetCurrentProcess();
            assert_eq!(GetPriorityClass(process), ABOVE_NORMAL_PRIORITY_CLASS.0);
            let (mut min, mut max, mut flags) = (0, 0, 0);
            GetProcessWorkingSetSizeEx(process, &mut min, &mut max, &mut flags).unwrap();
            assert_eq!(min, super::RESIDENT_BYTES);
            assert_ne!(flags & QUOTA_LIMITS_HARDWS_MIN_ENABLE.0, 0, "memory stays resident");
        }
    }
}
