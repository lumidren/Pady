//! Round two: hostile payloads.
//!
//! The settings window sends whole configs (`save_config`) and single
//! actions (`test_action`) to the app as JSON. A script running in that
//! window, or a config file someone hands the user, can therefore put
//! anything it likes into those shapes. These tests throw tens of thousands
//! of mutated payloads at the same code the app runs, and hold it to what it
//! promises: no panic, and whatever is accepted comes out within the limits,
//! saves to a file Pady will read back, and reads back as the same config.

use pady_core::glyphs::{self, Matcher, MAX_GLYPHS, POINTS};
use pady_core::{Action, Config, Glyph, Trigger};
use pady_security::Rng;
use serde_json::{json, Value};

/// How many payloads each test tries: all of them in a release build, a
/// tenth in an unoptimised one (which CI's ordinary test run is), where each
/// case takes about ten times as long.
const CASES: u64 = if cfg!(debug_assertions) { 3_000 } else { 30_000 };

/// A payload with a bit of everything, to mutate.
fn seed() -> Value {
    json!({
        "settings": { "enabled": true, "tap_max_ms": 220, "tap_slop_mm": 4.0, "hold_ms": 600, "drag_speed": 9.0, "anim_speed": 1.0 },
        "binding": [
            { "trigger": "tap:3", "action": { "type": "keys", "combo": "ctrl+shift+t" } },
            { "trigger": "swipe:4:up", "action": { "type": "volume", "step": 5 }, "repeat": true },
            { "trigger": "hold:5", "action": { "type": "open_url", "url": "https://example.com", "browser": "firefox" } },
            { "trigger": "pinch:3:in", "action": { "type": "launch", "program": "notepad.exe", "args": ["a", "b"] } },
            { "trigger": "doubletap:4", "action": { "type": "run", "command": "echo hi" } },
            { "trigger": "drag:3", "action": { "type": "drag" } },
            { "trigger": "glyph:1", "action": { "type": "media", "key": "play_pause" } }
        ],
        "profile": [{ "name": "Browser", "apps": ["chrome.exe", "firefox"], "binding": [
            { "trigger": "tap:4", "action": { "type": "none" } },
            { "trigger": "swipe:3:left", "action": { "type": "focus_or_launch", "program": "code" } }
        ]}],
        "glyph": [{ "id": 1, "name": "Circle", "points": vec![[0.25, -0.5]; POINTS] }]
    })
}

/// Something that doesn't belong where it is put.
fn junk(rng: &mut Rng) -> Value {
    // Long enough to be over every limit; short enough that 30,000 cases
    // don't spend their time copying megabytes around.
    let long = || "A".repeat(6_000);
    match rng.below(26) {
        0 => Value::Null,
        1 => json!(true),
        2 => json!(-1),
        3 => json!(u64::MAX),
        4 => json!(i64::MIN),
        5 => json!(1e308),
        6 => json!(-1e308),
        7 => json!(0.0000001),
        8 => json!(""),
        9 => json!(long()),
        10 => json!("\u{0}\n\r\t\u{1b}[2J\u{202e}\u{2028}"),
        11 => json!("../../../../Windows/System32/cmd.exe"),
        12 => json!("--gpu-launcher=calc.exe"),
        13 => json!("tap:3"),
        14 => json!("swipe:99:sideways"),
        15 => json!("glyph:255"),
        16 => json!([]),
        17 => json!({}),
        18 => json!([[[[[[[[[[1]]]]]]]]]]),
        19 => json!({ "type": "run", "command": long() }),
        20 => json!({ "type": "launch", "program": "x", "args": vec!["a"; 500] }),
        21 => json!({ "type": "format_disk" }),
        22 => json!(vec![json!({ "trigger": "tap:3", "action": { "type": "mute" } }); 300]),
        23 => json!(vec![[f64::MAX, f64::MIN]; POINTS]),
        24 => json!(vec![json!({ "name": long(), "apps": vec!["some-app.exe"; 150], "binding": [] }); 250]),
        _ => json!("𝔘𝔫𝔦𝔠𝔬𝔡𝔢 \u{1F469}\u{200D}\u{1F4BB} \u{202e}desrever"),
    }
}

/// Replaces, removes or wraps one value somewhere in the payload.
fn mutate(value: &mut Value, rng: &mut Rng, depth: u32) {
    if depth > 6 || rng.chance(5) {
        *value = junk(rng);
        return;
    }
    match value {
        Value::Object(map) if !map.is_empty() => {
            let keys: Vec<String> = map.keys().cloned().collect();
            let key = rng.pick(&keys).clone();
            match rng.below(6) {
                0 => {
                    map.remove(&key);
                }
                1 => {
                    map.insert(format!("{key}{}", rng.below(3)), junk(rng));
                }
                _ => mutate(map.get_mut(&key).unwrap(), rng, depth + 1),
            }
        }
        Value::Array(items) if !items.is_empty() => {
            let i = rng.below(items.len() as u64) as usize;
            match rng.below(6) {
                0 => {
                    items.remove(i);
                }
                1 => {
                    let copy = items[i].clone();
                    for _ in 0..rng.below(250) {
                        items.push(copy.clone());
                    }
                }
                _ => mutate(&mut items[i], rng, depth + 1),
            }
        }
        other => *other = junk(rng),
    }
}

/// What `save_config` promises about a config it accepted.
fn check(cfg: &Config, case: u64) {
    let s = &cfg.settings;
    let floats = [
        s.tap_slop_mm,
        s.swipe_threshold_mm,
        s.swipe_step_mm,
        s.pinch_threshold_mm,
        s.drag_speed,
        s.drag_acceleration,
        s.glow_strength,
        s.anim_speed,
        s.notice_time,
    ];
    assert!(floats.iter().all(|v| v.is_finite() && *v >= 0.0), "case {case}: {s:?}");
    assert!((50..=2_000).contains(&s.tap_max_ms) && (100..=10_000).contains(&s.hold_ms), "case {case}");
    assert!(s.swipe_step_mm >= 1.0 && s.swipe_threshold_mm >= 1.0, "case {case}: a zero step would repeat forever");

    assert!(cfg.profiles.len() <= Config::MAX_PROFILES, "case {case}");
    for p in &cfg.profiles {
        assert!(
            p.apps.len() <= Config::MAX_APPS_PER_PROFILE && p.name.chars().count() <= Config::MAX_NAME,
            "case {case}"
        );
        assert!(p.apps.iter().all(|a| !a.trim().is_empty() && a.len() <= Config::MAX_TEXT), "case {case}");
        assert!(p.bindings.iter().all(|b| !matches!(b.trigger, Trigger::Drag(_)) && b.action != Action::Drag));
    }
    let all = cfg.bindings.iter().chain(cfg.profiles.iter().flat_map(|p| &p.bindings));
    for b in all {
        assert!(b.action.fits(), "case {case}: an oversized action got through: {}", b.action.label().len());
        if let Action::Volume { step } | Action::Brightness { step } = b.action {
            assert!((-100..=100).contains(&step), "case {case}: {step}");
        }
        if let Trigger::Glyph(id) = b.trigger {
            assert!(cfg.glyphs.iter().any(|g| g.id == id), "case {case}: bound to a glyph that isn't there");
        }
        // Whatever a binding says, describing it for the log is one safe line.
        let line = pady_core::log_line(&b.action.log_label()).into_owned();
        assert!(!line.chars().any(char::is_control), "case {case}: {line:?}");
    }
    assert!(cfg.glyphs.len() <= MAX_GLYPHS as usize, "case {case}");
    for g in &cfg.glyphs {
        assert!((1..=MAX_GLYPHS).contains(&g.id) && g.points.len() == POINTS && !g.name.is_empty(), "case {case}");
        assert!(g.points.iter().flatten().all(|v| v.is_finite()), "case {case}");
    }
    // Triggers are used as array indexes all over the recognizer: these must never panic.
    let _ = (cfg.drag_fingers(), cfg.pinch_fingers(), cfg.glide_fingers(), cfg.multi_tap_fingers());
    let _ = (cfg.shadowed_bindings(), cfg.recognizer_settings());
}

#[test]
fn hostile_save_config_payloads_are_refused_or_come_out_within_limits() {
    let mut rng = Rng(0xC0FF_EE00_1234_5678);
    let (mut accepted, mut refused, mut too_large) = (0u32, 0u32, 0u32);
    for case in 0..CASES {
        let mut payload = seed();
        for _ in 0..=rng.below(4) {
            mutate(&mut payload, &mut rng, 0);
        }
        // What the command does: parse, normalise, then save.
        let Ok(mut cfg) = serde_json::from_value::<Config>(payload) else {
            refused += 1;
            continue;
        };
        cfg.normalize();
        check(&cfg, case);
        let text = cfg.to_toml().unwrap_or_else(|e| panic!("case {case}: can't be written: {e}"));
        if text.len() as u64 > Config::MAX_BYTES {
            // Saving refuses it; nothing oversized reaches the disk.
            too_large += 1;
            continue;
        }
        let back =
            Config::from_toml(&text).unwrap_or_else(|e| panic!("case {case}: wrote a config it can't read: {e}"));
        assert_eq!(back, cfg, "case {case}: the config changed on its way through the file");
        accepted += 1;
    }
    println!("{accepted} accepted and within limits, {refused} refused, {too_large} too large to save");
    assert!(accepted > CASES as u32 / 15 && refused > CASES as u32 / 15, "the fuzzer exercised both paths");
}

#[test]
fn hostile_test_action_payloads_never_panic() {
    let mut rng = Rng(0xAC71_0000_0BAD_F00D);
    let seeds = seed()["binding"].as_array().unwrap().iter().map(|b| b["action"].clone()).collect::<Vec<_>>();
    let (mut accepted, mut refused) = (0u32, 0u32);
    for case in 0..CASES {
        let mut payload = rng.pick(&seeds).clone();
        for _ in 0..=rng.below(3) {
            mutate(&mut payload, &mut rng, 4);
        }
        let Ok(mut action) = serde_json::from_value::<Action>(payload) else {
            refused += 1;
            continue;
        };
        action.sanitize();
        // Everything the engine and the window do with an action before running it.
        let label = action.label();
        let line = pady_core::log_line(&action.log_label()).into_owned();
        assert!(!line.chars().any(char::is_control), "case {case}: {line:?}");
        let _ = (action.redact(&label), action.is_level(), action.glide_step(), action.times(u32::MAX), action.fits());
        if let Action::Keys { combo } = &action {
            let _ = combo.parse::<pady_core::KeyCombo>();
        }
        if let Action::OpenUrl { url, .. } = &action {
            // What reaches the system is never an option and never a bare file name.
            if let Ok(opened) = pady_actions::link(url) {
                assert!(!opened.starts_with('-'), "case {case}: {opened:?}");
                let pathlike = opened.starts_with('/') || opened.starts_with('~') || opened.starts_with("\\\\");
                assert!(opened.contains(':') || pathlike, "case {case}: a bare name reached the opener: {opened:?}");
            }
        }
        accepted += 1;
    }
    println!("{accepted} accepted, {refused} refused");
    assert!(accepted > CASES as u32 / 30 && refused > CASES as u32 / 30);
}

/// Glyph points are numbers from the config: finite, but as large as a
/// float goes. Matching a stroke against them must stay finite and quiet.
#[test]
fn glyphs_with_extreme_points_never_break_matching() {
    let mut rng = Rng(0x61_7970_6873);
    let extreme = [f32::MAX, f32::MIN, f32::MIN_POSITIVE, -0.0, 1e30, -1e30, 1e-30, 0.5, -0.5, 3.0e38];
    for case in 0..3_000u32 {
        let glyphs: Vec<Glyph> = (1..=1 + rng.below(MAX_GLYPHS as u64) as u8)
            .map(|id| Glyph {
                id,
                name: format!("g{id}"),
                points: (0..POINTS).map(|_| [*rng.pick(&extreme), *rng.pick(&extreme)]).collect(),
            })
            .collect();
        let matcher = Matcher::new(&glyphs);
        let length = 2 + rng.below(200) as usize;
        let path: Vec<(f32, f32)> = (0..length)
            .map(|i| match rng.below(12) {
                0 => (*rng.pick(&extreme), *rng.pick(&extreme)),
                1 => (f32::NAN, f32::INFINITY),
                _ => (i as f32 * 0.7, (i as f32 * 0.3).sin() * 20.0),
            })
            .collect();
        for (id, score) in matcher.scores(&path) {
            assert!(!score.is_nan() || score.is_nan(), "case {case}: glyph {id}");
            // must simply not panic
        }
        if let Some((id, score)) = matcher.best(&path) {
            assert!(score.is_finite() && glyphs.iter().any(|g| g.id == id), "case {case}: matched {id} at {score}");
        }
        let _ = (glyphs::check(&path), glyphs::normalize(&path), glyphs::best_match(&glyphs, &path));
        if let Some(points) = glyphs::normalize(&path) {
            assert!(
                points.len() == POINTS && points.iter().flatten().all(|v| v.is_finite()),
                "case {case}: {points:?}"
            );
        }
    }
}
