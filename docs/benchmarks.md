# Pady benchmarks

Measured 16 November 2026, before and after the changes that followed the security and performance assessment of 1.1.1 ([security/REPORT.md](../security/REPORT.md)).

| | |
|---|---|
| Machine | Acer Nitro AN515-58, Intel Core i7-12700H (14 cores), 16 GB RAM, on mains power |
| System | Windows 11 Home, build 26200 |
| Toolchain | rustc 1.99.1, release profile (`lto = true`, `codegen-units = 1`) |
| Before | v1.1.1, commit `c17f9d3` |
| After | v1.1.2 |
| Runs | Per-frame numbers (`cargo bench -p pady-core`) are medians of 3 runs. The load harness ran three times on each version; the latency tables show the first run of the current code, and profile and config numbers are the median of the three. Start-up timings are medians of 35. |

No real touchpad or real actions are involved: frames are generated, and actions go to a stand-in that records when it was called. A real touchpad sends 125–250 frames a second.

## How to run them

```bash
cargo bench -p pady-core                              # per-frame cost, gesture → action latency
cargo bench -p pady-security                          # load ramp, overload, profiles, config I/O, spike
PADY_SOAK_MINUTES=30 cargo bench -p pady-security     # the same plus a 30-minute soak
cargo test --release -p pady-security --test perf_regression -- --nocapture   # the regression gate
```

## What changed

| | Before (1.1.1) | After | |
|---|---|---|---|
| Fast 400 mm volume swipe, 5 ms per step: calls made | 195 | 2 | steps that queue up run as one change |
| …and how long the action thread stays busy after the lift | 1.07 s | 11 ms | 97× sooner |
| A device racing its fingers (8,000 queued steps) | 44 s busy | 13 ms | |
| Profile lookup per gesture, 200 profiles × 20 apps | 303 µs | 75 µs | 4× faster, no allocation |
| Profile lookup, 10 profiles | 14.6 µs | 3.4 µs | |
| Two saves at once | 11 torn configs in 4 s | 0 | own temp file, flushed |
| Saving the config (2.7 KB) | 0.73 ms | 1.56 ms | the price of flushing to disk before the rename |
| A 64 MB config file | read (87 ms), copied to backups | refused before reading | 4 MB limit |

## Per touch frame (`cargo bench -p pady-core`)

Time and heap allocations for one frame through the recognizer.

| Gesture | Before | After | Allocations per frame |
|---|---|---|---|
| 1 finger pointing | 25 ns | 26 ns | 0 |
| 2 fingers scrolling | 56 ns | 59 ns | 0 |
| 4-finger swipe with repeats | 117 ns | 124 ns | 0.21 |
| 3-finger drag | 119 ns | 120 ns | 1.00 |
| 4-finger pinch with repeats | 110 ns | 110 ns | 0.03 |

Nothing in the recognizer changed; the differences are run-to-run noise (about ±5%).

| From … to the action running | Before p50 | Before p99 | After p50 | After p99 |
|---|---|---|---|---|
| Five-finger tap lifts | 62.7 µs | 177.8 µs | 56.5 µs | 179.2 µs |
| "Test" button in the window | 37.9 µs | 160.0 µs | 36.8 µs | 142.0 µs |

## Load: raising the frame rate (`cargo bench -p pady-security`)

Three-finger taps offered at a steady rate for 3 seconds per row. Latency is from the frame that completes the tap (the lift) to its action running, on the current code.

| Offered | Frames/s | p50 | p95 | p99 | Worst |
|---|---|---|---|---|---|
| 1 touchpad | 125 | 47 µs | 185 µs | 256 µs | 285 µs |
| 10 touchpads | 1,250 | 127 µs | 205 µs | 270 µs | 916 µs |
| 50 touchpads | 6,250 | 20 µs | 78 µs | 125 µs | 673 µs |
| 100 touchpads | 12,500 | 14 µs | 49 µs | 94 µs | 355 µs |
| 500 touchpads | 62,500 | 13 µs | 30 µs | 63 µs | 433 µs |

Latency *falls* as the rate rises: at one touchpad's rate the engine's threads go to sleep between frames, and most of the 47 µs is Windows waking them. Under load they stay awake. Nothing broke at 500 times a touchpad's rate; 1.1.1 measured the same within noise.

### Where it breaks

| | |
|---|---|
| Recognizer alone, mixed gestures | 94 ns per frame: 10.6 million frames/s on one core |
| Whole engine (recognizer, queue, action thread), taps | 6.8 million frames/s, every gesture's action run |
| Offered 23 million frames/s (as fast as one thread can send) | the queue between input and engine grew to 1.4 million frames; memory went from 19 MB to 125 MB, and back once it had caught up 0.2 s later |

What gives first is memory: the queue from the touchpad reader to the engine is unbounded, so frames arriving faster than 6.8 million a second pile up until the engine catches up. That is 27,000 times what a touchpad sends and far beyond what USB can carry (8,000 reports a second), so no real device can get there. No gesture was lost.

## Spike and recovery

500,000 frames (250,000 taps) sent at once, then a tap every 16 ms.

| | Before | After |
|---|---|---|
| Burst worked off in | 69 ms | 73 ms |
| Latency straight afterwards, p50 / p99 / worst | 88 / 214 / 273 µs | 69 / 228 / 283 µs |

Back to normal latency immediately, with no restart and no backlog left over.

## Failure under load

These are tests rather than timings (`cargo test -p pady-security`):

| What was done | Result |
|---|---|
| A process killed at a random moment while saving, 25 times | The config read back whole every time; unfinished temp files are removed by a later save |
| A save while another program holds the config locked | The save fails cleanly, the old config stays, no temp file is left |
| Six threads saving three configs for 4 seconds | 0 torn reads, 0 failed saves |
| 20,000 rounds of the per-gesture system calls (app in front, full-screen check, registry, program lookup) | Handles 209 → 209, memory +108 KB |
| A million frames of random gestures, one in fifty frames garbage | Handles 155 → 155, memory flat |

Filling the disk was not tested: it needs a small dedicated volume, which wasn't available.

## Soak

`PADY_SOAK_MINUTES=30 cargo bench -p pady-security`, on the current code: 30 minutes at about 1,000 frames a second (eight touchpads' worth) of taps and repeating swipes, with the config changed every second and saved and backed up every five.

| Minute | Memory (committed) | Handles | Taps so far | Actions run | Saves |
|---|---|---|---|---|---|
| 0 | 20,176 KB | 87 | 100 | 236 | 0 |
| 1 | 20,184 KB | 87 | 6,000 | 14,773 | 12 |
| 5 | 20,248 KB | 87 | 30,000 | 73,834 | 60 |
| 10 | 21,028 KB | 87 | 60,000 | 135,667 | 120 |
| 15 | 20,996 KB | 87 | 90,000 | 189,672 | 180 |
| 20 | 20,996 KB | 87 | 120,000 | 250,591 | 240 |
| 25 | 20,996 KB | 87 | 150,000 | 313,294 | 300 |
| 29 | 20,996 KB | 87 | 174,000 | 362,627 | 348 |

- 1,785,000 frames in 1,801 seconds: 178,500 taps and 35,700 repeating swipes ran 370,855 actions. Every tap ran exactly one action and every swipe at least its first step: nothing was lost.
- Memory rose by 0.8 MB in the first seven minutes and then did not move for the remaining 23. Handles stayed at 87 throughout.
- The config was saved 357 times and read back whole each time; 15 backups were kept (one per two minutes, as designed).


## Config on disk

Each number is the median of 20 operations per run, and then the median of three runs, on an SSD with the system's virus scanner active.

| | Size | Save | Load | Backup copy |
|---|---|---|---|---|
| Default config, before | 2.7 KB | 0.73 ms | 0.81 ms | 1.31 ms |
| Default config, after | 2.7 KB | 1.56 ms | 0.88 ms (see below) | 1.73 ms |
| 200 profiles × 20 apps, before | 98 KB | 3.18 ms | 15.5 ms | 13.8 ms |
| 200 profiles × 20 apps, after | 98 KB | 4.38 ms | 15.5 ms | 13.6 ms |

Saving costs about 0.3–0.8 ms more than before because the file is now flushed to disk before it replaces the old one. Saves happen when a setting changes, never while a gesture is being recognised.

**A caveat on "load".** Opening a file that was written a moment ago normally costs about 0.7 ms here, most of it the virus scanner's first look; a second load of the same file takes 0.17 ms. In two of the three runs of the current code the default config's load read 12–13 ms instead of 0.88 ms. A direct A/B (the old way of saving against the new, 20 saves each, three rounds) showed the same 13 ms opens with the *old* way in one round and never with the new, so this is the scanner inspecting new files in a fresh folder, not the change in how configs are saved.

## Profile lookup

Time to find the binding for the app in front when it is in no profile (the worst case: every app name is compared).

| Profiles (20 apps each) | Before | After |
|---|---|---|
| 1 | 1.40 µs | 0.32 µs |
| 10 | 14.6 µs | 3.4 µs |
| 50 | 70.3 µs | 16.8 µs |
| 100 | 138 µs | 33.7 µs |
| 200 (the limit) | 303 µs | 74.7 µs |

The three runs at 200 profiles measured 65, 75 and 126 µs after, and 303 and 295 µs before.

## Size and start-up

| | Before | After |
|---|---|---|
| `pady.exe` (release, before bundling) | 14,955,008 bytes | 15,004,672 bytes (+0.3%) |
| `pady-cli.exe` | 2,911,744 bytes | 2,956,288 bytes (+1.5%) |
| `pady-cli presets`, start to exit (median of 35) | 11.8 ms | 11.8 ms |
| `pady-cli config path` | 13.7 ms | 13.1 ms |
| Settings UI: files loaded | 6 files, 365 KB uncompressed | the same |
| Settings UI: load event, first / repeat (served locally to a browser, with the built-in demo backend) | – | 148 ms / 15–26 ms |
| Settings UI: elements on the Gestures page | – | 578 |

## Not measured

- **The app's own start-up time and memory.** Measuring them means stopping the copy of Pady that is in use on the test machine. The README's figures (about 20 MB in the tray, 0% CPU idle) are from 1.1.0 and nothing in this work touches start-up.
- **Frame rate of the settings UI's animations and long lists.** The test browser's window was hidden during the run, which pauses animation frames.
- **Linux.** Everything here ran on Windows. On Linux each volume or brightness step starts a helper program (10–30 ms), so the queueing fix matters more there than these Windows numbers show.
- **Battery use.**

## The regression gate

CI runs `security/tests/perf_regression.rs` on every push and fails when:

- heap allocations per 1,000 frames, for any kind of gesture, are more than 20% over `security/perf-baseline.json` (pointing, scrolling, taps and the profile lookup must stay at exactly zero);
- time per frame is more than 50% over the baseline on CI, or 20% on the machine the baseline was recorded on. Time is measured relative to a fixed piece of arithmetic timed in the same run, so a slower machine doesn't fail; but different processor models still shift that ratio by up to about 20% with no code change (measured on GitHub's runners), which is why CI's limit is wider.

| Baseline (after) | Allocations per 1,000 frames | Time per frame, in yardsticks |
|---|---|---|
| 1 finger pointing | 0 | 7.2 |
| 2 fingers scrolling | 0 | 16.2 |
| 3-finger tap | 0 | 15.4 |
| 4-finger repeating swipe | 609.8 | 40.0 |
| 3-finger drag | 967.2 | 34.1 |
| Profile lookup (20 profiles × 10 apps) | 0 | 918 |

Each timing is the median of 31 repetitions, and in each repetition the yardstick and the work are timed back to back, so both run on the same core at the same clock speed. A number over its limit is measured again, twice at most, before the test fails: a real regression is slow every time, a busy moment is not. Six runs on the reference machine stayed within 6% of these values (11% for the tap).
