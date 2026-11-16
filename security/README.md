# Pady's security tests

Every test in `tests/` is an attack. Each one first failed against the code as it was, proving a weakness, and passes now that it's fixed. They run with the rest of the workspace's tests in CI, on Windows and Linux, on every push.

```bash
cargo test -p pady-security
```

That's the whole suite: under a minute. Tests only touch throwaway folders under the system temp directory and remove them again. On Windows two of them start a harmless copy of `whoami.exe` from such a folder, so a console window may flash.

| File | What it attacks | Finding |
|---|---|---|
| `tests/windows_launch.rs` | A program planted in the current folder, started by its bare name | PADY-03 |
| `tests/ci_pipeline.rs` | Workflows: unpinned actions, token permissions, where secrets reach | PADY-08 |
| `tests/config_integrity.rs` | Six threads saving the config at once | PADY-01 |
| `tests/config_crash.rs` | A process killed mid-save; a save blocked by a locked file | PADY-01 |
| `tests/backups.rs` | Backups dated in the future; hostile backup file names | PADY-02 |
| `tests/windows_run.rs` | Shell commands with quotes and special characters | PADY-04 |
| `tests/log_hygiene.rs` | Keys and tokens in the log; forged log lines | PADY-05 |
| `tests/config_limits.rs` | A 64 MB config; thousands of profiles; deeply nested TOML | PADY-06 |
| `tests/window_policy.rs` | The settings window's policy and every place text could become HTML | PADY-07 |
| `tests/site.rs` | The website turning GitHub's API answer into HTML | PADY-09 |
| `tests/action_flood.rs` | Level changes queued faster than they can run | PADY-10 |
| `tests/binary_hardening.rs` | The built program's exploit protections (Control Flow Guard, ASLR, DEP) | PADY-14 |
| `tests/payload_fuzz.rs` | 60,000 mutated `save_config` and `test_action` payloads; glyphs with extreme points | sound; kept that way |
| `tests/updater_config.rs` | The updater's endpoints, key and checks | sound; kept that way |
| `tests/privacy.rs` | Reading the clipboard or keyboard; new network code | sound; kept that way |
| `tests/leaks.rs` | Handle and memory leaks over 20,000 rounds and a million frames | sound; kept that way |
| `tests/perf_regression.rs` | Allocations per frame over 20% worse than `perf-baseline.json` (everywhere), or time per frame over 20% (on the reference machine) | see `docs/benchmarks.md` |

The findings are written up in [REPORT.md](REPORT.md), and what the tests defend against in [THREAT-MODEL.md](THREAT-MODEL.md).

## Load, spike and soak

```bash
cargo bench -p pady-security                          # about a minute
PADY_SOAK_MINUTES=30 cargo bench -p pady-security     # plus a 30-minute soak
```

`benches/load.rs` ramps the frame rate from one touchpad's worth to 500, finds the point where the engine can't keep up, times profile lookup and config I/O, and checks recovery after a burst of half a million frames. The soak keeps a steady load going while the config changes and is saved, and fails if a gesture goes missing or memory or handles grow. Results are in [`docs/benchmarks.md`](../docs/benchmarks.md).

## Performance baseline

`tests/perf_regression.rs` compares against `perf-baseline.json`:

- heap allocations per 1,000 frames, counted exactly (in every build);
- time per frame relative to a fixed piece of arithmetic timed in the same run (release builds only). The 20% limit is enforced on the machine the baseline was recorded on. CI prints the timings without enforcing them: on GitHub's machines the same code measured from 13% under to 64% over the baseline. Run it locally before a release (`RELEASING.md`).

After a deliberate change to these numbers, record a new baseline on a quiet machine and commit it:

```bash
PADY_PERF_RECORD=1 cargo test --release -p pady-security --test perf_regression
```

## Adding a test

1. Write the attack as a test and run it against the unfixed code. It must fail.
2. Fix the code. The same test must pass.
3. Say in the file's first comment what was wrong, in a sentence someone new can follow.
