//! How long switching the keyboard language takes, from the request to the
//! app in front using the new layout: Pady's "Switch keyboard language"
//! (Windows' language hotkey, no pop-up) against a plain Win+Space (which
//! opens Windows' input switcher). Switches an even number of times, so the
//! language ends as it started.
//!
//!     cargo run -p pady-actions --release --example lang_switch

#[cfg(windows)]
fn main() {
    use pady_core::Action;
    use std::time::{Duration, Instant};
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    fn layout() -> isize {
        unsafe { GetKeyboardLayout(GetWindowThreadProcessId(GetForegroundWindow(), None)).0 as isize }
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
    fn wait_change(from: isize, start: Instant) -> Option<Duration> {
        while start.elapsed() < Duration::from_secs(2) {
            if layout() != from {
                return Some(start.elapsed());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        None
    }

    if unsafe { GetKeyboardLayoutList(None) } < 2 {
        println!("only one keyboard language: nothing to switch");
        return;
    }
    let mut exec = pady_actions::executor();
    let switch = Action::Keys { combo: "win+space".into() };
    let start_layout = layout();
    for round in 0..2 {
        let (from, start) = (layout(), Instant::now());
        let result = exec.run(&switch);
        println!("round {round}: Pady's switch   → {:?} ({result:?})", wait_change(from, start));
        std::thread::sleep(Duration::from_millis(700));

        let (from, start) = (layout(), Instant::now());
        let inputs = [key(VK_LWIN, false), key(VK_SPACE, false), key(VK_SPACE, true), key(VK_LWIN, true)];
        unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        println!("round {round}: plain Win+Space → {:?}", wait_change(from, start));
        std::thread::sleep(Duration::from_millis(900));
    }
    println!("app in front: started {start_layout:#x}, ends {:#x}", layout());
}

#[cfg(not(windows))]
fn main() {}
