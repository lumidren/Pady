//! PADY-14: Pady's Windows programs were built without Control Flow Guard.
//!
//! Pady is written in Rust, but about 150 blocks of it call Windows
//! directly (`unsafe`), and a memory bug there is where an attacker would
//! start. Windows offers programs three protections that make such a bug
//! much harder to turn into running code. Two were on: address space layout
//! randomization and data execution prevention. The third, Control Flow
//! Guard, which stops a corrupted function pointer from jumping anywhere
//! but a real function's start, was not: Rust leaves it off unless asked.
//!
//! This test reads the header of the very program it runs in, which is
//! built with the same settings as `pady.exe`.
#![cfg(windows)]

const HIGH_ENTROPY_VA: u16 = 0x0020;
const DYNAMIC_BASE: u16 = 0x0040;
const NX_COMPAT: u16 = 0x0100;
const GUARD_CF: u16 = 0x4000;

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

/// The program's `DllCharacteristics` flags and its sections' (name, flags).
fn header(data: &[u8]) -> (u16, Vec<(String, u32)>) {
    let pe = u32_at(data, 0x3C) as usize;
    assert_eq!(&data[pe..pe + 4], b"PE\0\0", "a Windows program");
    let sections = u16_at(data, pe + 6) as usize;
    let optional = pe + 24;
    assert_eq!(u16_at(data, optional), 0x20B, "a 64-bit program");
    let table = optional + u16_at(data, pe + 20) as usize;
    let list = (0..sections)
        .map(|i| {
            let entry = table + 40 * i;
            let name = String::from_utf8_lossy(&data[entry..entry + 8]).trim_end_matches('\0').to_string();
            (name, u32_at(data, entry + 36))
        })
        .collect();
    (u16_at(data, optional + 70), list)
}

#[test]
fn windows_programs_are_built_with_every_exploit_protection() {
    let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
    let (flags, sections) = header(&me);
    for (flag, name) in [
        (DYNAMIC_BASE, "address space layout randomization"),
        (HIGH_ENTROPY_VA, "64-bit address space layout randomization"),
        (NX_COMPAT, "data execution prevention"),
        (GUARD_CF, "Control Flow Guard (see .cargo/config.toml)"),
    ] {
        assert!(flags & flag != 0, "built without {name}");
    }
    // Memory that is both writable and executable would undo all of the above.
    const EXECUTE: u32 = 0x2000_0000;
    const WRITE: u32 = 0x8000_0000;
    for (name, flags) in sections {
        assert!(flags & EXECUTE == 0 || flags & WRITE == 0, "section {name} is writable and executable");
    }
}

/// The setting applies to release builds of the app too: it lives in the
/// workspace's Cargo configuration, which every `cargo` and `tauri build`
/// run in this repository reads.
#[test]
fn the_setting_covers_every_build() {
    let config = pady_security::read(".cargo/config.toml");
    let section = config.split("[target.x86_64-pc-windows-msvc]").nth(1).expect("a Windows section");
    assert!(section.contains("control-flow-guard"), "{section}");
    // An environment variable would silently replace the config's flags.
    for workflow in ["ci.yml", "release.yml"] {
        let text = pady_security::read(&format!(".github/workflows/{workflow}"));
        assert!(!text.contains("RUSTFLAGS"), "{workflow} sets RUSTFLAGS, which overrides .cargo/config.toml");
    }
}
