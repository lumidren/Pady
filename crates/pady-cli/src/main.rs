//! `pady-cli`: a terminal companion to the Pady app.
//!
//! ```text
//! pady-cli devices          list detected touchpads
//! pady-cli monitor          show fingers and recognised gestures live (no actions run)
//! pady-cli run              run Pady headless with your config (actions run)
//! pady-cli action <toml>    run one action now, e.g. '{ type = "volume", step = 5 }'
//! pady-cli app              print the app in front as you switch, for per-app profiles
//! pady-cli doctor           check that everything Pady needs is in place
//! pady-cli fix-permissions  Linux: install the udev rule Pady needs (asks for a password)
//! pady-cli presets          list starter presets
//! pady-cli config [path|show|reset|preset <id>]
//! ```

use crossbeam_channel::unbounded;
use pady_core::engine::LogExecutor;
use pady_core::{Action, Config, Engine, EngineEvent, Executor, Gesture, Level, LevelKind, SharedConfig};
use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

const USAGE: &str = "\
pady-cli — touchpad gestures for Windows and Linux

USAGE:
    pady-cli devices          List detected touchpads
    pady-cli monitor          Show fingers and recognised gestures live (no actions run)
    pady-cli run              Run Pady headless with your config (actions run)
    pady-cli action <toml>    Run one action now, e.g. pady-cli action '{ type = \"volume\", step = 5 }'
    pady-cli app              Print the app in front as you switch (names for per-app profiles)
    pady-cli doctor           Check that everything Pady needs is in place
    pady-cli fix-permissions  Linux: let Pady read the touchpad and send keys (asks for a password)
    pady-cli config path      Print the config file location
    pady-cli config show      Print the active config
    pady-cli config reset     Overwrite the config with defaults
    pady-cli presets          List starter presets
    pady-cli config preset <id>  Replace your All-apps bindings with a preset

Set RUST_LOG=debug for more detail.";

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["devices"] => devices(),
        ["monitor"] => watch(false),
        ["run"] => watch(true),
        ["action", toml] => action(toml),
        ["app"] => foreground_apps(),
        ["doctor"] => doctor(),
        ["fix-permissions"] => fix_permissions(),
        ["config", "path"] | ["config"] => config_path(),
        ["config", "show"] => config_show(),
        ["config", "reset"] => config_reset(),
        ["presets"] => list_presets(),
        ["config", "preset", id] => apply_preset(id),
        [] | ["help"] | ["-h"] | ["--help"] => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(format!("unknown command\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

type CliResult = Result<(), String>;

fn load_config() -> Result<(std::path::PathBuf, Config), String> {
    let path = Config::default_path().map_err(|e| e.to_string())?;
    let cfg = Config::load_or_create(&path).map_err(|e| e.to_string())?;
    Ok((path, cfg))
}

fn fix_permissions() -> CliResult {
    pady_actions::fix_permissions().map_err(|e| e.to_string())?;
    println!("Done. Pady can now read the touchpad and send keys; restart Pady if it is running.");
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Check {
    Ok,
    Problem,
    Info,
}

/// One screen answering "why doesn't my gesture work?": config, touchpad,
/// OS gestures that clash, app detection for profiles, and autostart.
fn doctor() -> CliResult {
    let mut problems = 0usize;
    let mut line = |check: Check, what: &str, detail: &str| {
        let mark = match check {
            Check::Ok => "✓",
            Check::Problem => {
                problems += 1;
                "✗"
            }
            Check::Info => "·",
        };
        let detail = detail.replace('\n', "\n                  ");
        println!("{mark} {what:<15} {detail}");
    };

    let path = Config::default_path().map_err(|e| e.to_string())?;
    let cfg = if !path.exists() {
        line(Check::Ok, "config", &format!("{} doesn't exist yet; defaults apply", path.display()));
        Config::default()
    } else {
        match Config::load(&path) {
            Ok(cfg) => {
                let counts = format!("{} bindings, {} profiles", cfg.bindings.len(), cfg.profiles.len());
                line(Check::Ok, "config", &format!("{} ({counts})", path.display()));
                cfg
            }
            Err(e) => {
                line(Check::Problem, "config", &e.to_string());
                Config::default()
            }
        }
    };

    let (tx, _rx) = unbounded();
    match pady_input::start(tx) {
        Ok(input) => {
            for d in input.devices() {
                let size = format!("{:.0} × {:.0} mm, up to {} fingers", d.width_mm, d.height_mm, d.max_contacts);
                line(Check::Ok, "touchpad", &format!("{} ({size})", d.name));
            }
        }
        Err(e) => line(Check::Problem, "touchpad", &e.to_string()),
    }

    let conflicts = pady_actions::conflicts(&cfg);
    if conflicts.is_empty() {
        line(Check::Ok, "os gestures", "nothing else reacts to your gestures");
    }
    for c in conflicts {
        line(Check::Problem, "os gestures", &format!("{}\n{}", c.message, c.fix));
    }

    match pady_actions::foreground_app() {
        Some(app) => line(Check::Ok, "app detection", &format!("works (the app in front is {app})")),
        None if cfg.profiles.is_empty() => {
            line(Check::Info, "app detection", "unavailable here; only per-app profiles need it")
        }
        None => line(Check::Problem, "app detection", "unavailable here, so your per-app profiles won't apply"),
    }

    let autostart = if pady_actions::autostart_enabled() { "on" } else { "off" };
    line(Check::Info, "autostart", &format!("{autostart} (change it in the app's Settings)"));

    println!();
    match problems {
        0 => {
            println!("Everything looks good.");
            Ok(())
        }
        n => Err(format!("{n} problem{} found", if n == 1 { "" } else { "s" })),
    }
}

/// Print the app in front whenever it changes, with the profile it would
/// use, so people can find the names to put in `apps = [...]`.
fn foreground_apps() -> CliResult {
    let (_, cfg) = load_config()?;
    println!("Switch between apps to see their names. Press Ctrl+C to stop.\n");
    let mut last: Option<String> = None;
    loop {
        let app = pady_actions::foreground_app();
        if app != last {
            match &app {
                Some(name) => {
                    let profile = cfg.profile_for(name).map(|p| format!("  → profile \"{}\"", p.name));
                    println!("{name}{}", profile.unwrap_or_default());
                }
                None => println!("(the OS doesn't say which app is in front)"),
            }
            last = app;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

fn devices() -> CliResult {
    let (tx, _rx) = unbounded();
    let input = pady_input::start(tx).map_err(|e| e.to_string())?;
    for d in input.devices() {
        println!("{}  —  {:.0} × {:.0} mm, up to {} fingers", d.name, d.width_mm, d.height_mm, d.max_contacts);
    }
    Ok(())
}

fn watch(run_actions: bool) -> CliResult {
    let (path, cfg) = load_config()?;
    let (tx, rx) = unbounded();
    let input = pady_input::start(tx).map_err(|e| e.to_string())?;
    for d in input.devices() {
        println!("touchpad: {} ({:.0} × {:.0} mm)", d.name, d.width_mm, d.height_mm);
    }
    println!("config:   {}", path.display());
    for c in pady_actions::conflicts(&cfg) {
        println!("warning:  {}\n          {}", c.message, c.fix);
    }
    println!(
        "{}  Press Ctrl+C to stop.\n",
        if run_actions {
            "Running — gestures trigger actions."
        } else {
            "Monitoring — gestures are shown, not run."
        }
    );

    let observer = Arc::new(|event: EngineEvent| match event {
        EngineEvent::Frame(frame) => {
            let fingers: Vec<String> =
                frame.contacts.iter().map(|c| format!("#{}({:>5.1},{:>5.1})", c.id, c.x, c.y)).collect();
            print!("\r\x1b[2K{} finger(s) {}", frame.contacts.len(), fingers.join(" "));
            let _ = std::io::stdout().flush();
        }
        EngineEvent::Gesture { gesture, trigger, action, profile } => {
            if matches!(gesture, Gesture::DragEnd) {
                println!("\r\x1b[2K→ drag released");
                return;
            }
            let what = trigger.unwrap_or_else(|| format!("{gesture:?}"));
            let did = action.map_or_else(|| "no action bound".to_string(), |a| format!("→ {a}"));
            let via = profile.map(|p| format!("  [{p}]")).unwrap_or_default();
            println!("\r\x1b[2K● {what:<16} {did}{via}");
        }
        EngineEvent::Level(level) => println!("\r\x1b[2K  {}", describe_level(&level)),
        EngineEvent::ActionFailed { action, error } => println!("\r\x1b[2K✗ {action} failed: {error}"),
    });

    let shared = Arc::new(SharedConfig::new(cfg));
    let engine = if run_actions {
        Engine::spawn_with(shared, rx, pady_actions::executor, observer, pady_input::sched::scheduling())
    } else {
        Engine::spawn(shared, rx, || Box::new(LogExecutor) as Box<dyn Executor>, observer)
    };
    engine.join();
    drop(input);
    Ok(())
}

fn action(toml_text: &str) -> CliResult {
    #[derive(serde::Deserialize)]
    struct Wrap {
        action: Action,
    }
    let wrap: Wrap = toml::from_str(&format!("action = {toml_text}")).map_err(|e| format!("invalid action: {e}"))?;
    println!("running: {}", wrap.action.label());
    let mut exec = pady_actions::executor();
    if let Some(level) = exec.run(&wrap.action).map_err(|e| e.to_string())? {
        println!("{}", describe_level(&level));
    }
    Ok(())
}

/// `volume 44% (muted)` — a text stand-in for the app's overlay.
fn describe_level(level: &Level) -> String {
    let kind = match level.kind {
        LevelKind::Volume => "volume",
        LevelKind::Brightness => "brightness",
    };
    let filled = (level.percent as usize + 5) / 10;
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled));
    format!("{kind:<10} {bar} {}%{}", level.percent, if level.muted { " (muted)" } else { "" })
}

fn config_path() -> CliResult {
    let path = Config::default_path().map_err(|e| e.to_string())?;
    println!("{}", path.display());
    Ok(())
}

fn config_show() -> CliResult {
    let (_, cfg) = load_config()?;
    print!("{}", cfg.to_toml().map_err(|e| e.to_string())?);
    Ok(())
}

fn list_presets() -> CliResult {
    for p in pady_core::PRESETS {
        println!(
            "{:<14} {}
{:<14} {}
",
            p.id, p.name, "", p.description
        );
    }
    Ok(())
}

fn apply_preset(id: &str) -> CliResult {
    let (path, mut cfg) = load_config()?;
    if !cfg.apply_preset(id) {
        return Err(format!("unknown preset `{id}`; see pady-cli presets"));
    }
    cfg.save(&path).map_err(|e| e.to_string())?;
    println!("applied the {id} preset to {}", path.display());
    Ok(())
}

fn config_reset() -> CliResult {
    let path = Config::default_path().map_err(|e| e.to_string())?;
    Config::default().save(&path).map_err(|e| e.to_string())?;
    println!("wrote defaults to {}", path.display());
    Ok(())
}
