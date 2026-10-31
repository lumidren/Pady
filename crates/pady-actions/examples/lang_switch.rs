//! How long switching the keyboard language takes, from the request to the
//! app in front using the new layout: Win+Space against asking the app's
//! window to switch directly. Switches an even number of times, so the
//! language ends as it started.
//!
//!     cargo run -p pady-actions --release --example lang_switch

#[cfg(windows)]
fn main() {
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    fn layout() -> isize {
        unsafe {
            let front = GetForegroundWindow();
            let thread = GetWindowThreadProcessId(front, None);
            GetKeyboardLayout(thread).0 as isize
        }
    }
    fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    ..Default::default()
                },
            },
        }
    }
    fn wait_change(from: isize) -> Option<Duration> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            if layout() != from {
                return Some(start.elapsed());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        None
    }

    let count = unsafe { GetKeyboardLayoutList(None) };
    if count < 2 {
        println!("only one keyboard language: nothing to switch");
        return;
    }
    println!("{count} keyboard languages; app in front starts with {:#x}", layout());
    for round in 0..2 {
        let from = layout();
        let inputs = [key(VK_LWIN, false), key(VK_SPACE, false), key(VK_SPACE, true), key(VK_LWIN, true)];
        unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        println!("round {round}: Win+Space        → {:?}", wait_change(from));
        std::thread::sleep(Duration::from_millis(900));

        let from = layout();
        unsafe {
            let _ = PostMessageW(
                GetForegroundWindow(),
                WM_INPUTLANGCHANGEREQUEST,
                WPARAM(INPUTLANGCHANGE_FORWARD as usize),
                LPARAM(1), // HKL_NEXT
            );
        }
        println!("round {round}: direct request   → {:?}", wait_change(from));
        std::thread::sleep(Duration::from_millis(600));
    }
    println!("app in front ends with {:#x}", layout());
}

#[cfg(not(windows))]
fn main() {}
