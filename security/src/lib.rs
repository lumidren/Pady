//! Helpers shared by Pady's security tests (`security/tests`).
//!
//! Every test here is an attack: it first proved a weakness against the code
//! as it was, and now proves the fix. Tests only ever touch throwaway folders
//! under the system temp directory, which they remove again.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// The repository root (the security crate lives directly below it).
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("security/ has a parent").to_path_buf()
}

/// A file from the repository, as text.
pub fn read(relative: &str) -> String {
    let path = repo().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A throwaway folder, removed when dropped.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Scratch {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("pady-security-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create a scratch folder");
        Scratch(dir)
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A small deterministic random number generator (xorshift64*), so a failing
/// fuzz case can be replayed from its seed.
pub struct Rng(pub u64);

impl Rng {
    pub fn roll(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number below `n` (`n` > 0).
    pub fn below(&mut self, n: u64) -> u64 {
        self.roll() % n
    }

    pub fn chance(&mut self, one_in: u64) -> bool {
        self.below(one_in) == 0
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

/// What this process holds right now: memory in bytes and open handles
/// (file descriptors on Linux). For leak checks.
#[derive(Clone, Copy, Debug)]
pub struct Footprint {
    pub memory: u64,
    pub handles: u64,
}

#[cfg(windows)]
pub fn footprint() -> Footprint {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    unsafe {
        let process = GetCurrentProcess();
        let mut counters = PROCESS_MEMORY_COUNTERS::default();
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let _ = GetProcessMemoryInfo(process, &mut counters, size);
        let mut handles = 0u32;
        let _ = GetProcessHandleCount(process, &mut handles);
        Footprint { memory: counters.PagefileUsage as u64, handles: u64::from(handles) }
    }
}

#[cfg(target_os = "linux")]
pub fn footprint() -> Footprint {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kb = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
        .unwrap_or(0);
    let handles = std::fs::read_dir("/proc/self/fd").map(|d| d.count() as u64).unwrap_or(0);
    Footprint { memory: kb * 1024, handles }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn footprint() -> Footprint {
    Footprint { memory: 0, handles: 0 }
}
