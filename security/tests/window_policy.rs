//! The settings window can run any action (`test_action`), so script
//! injected into it would be code execution. These tests pin down what
//! keeps that from happening.
//!
//! PADY-07: the Content Security Policy had no `base-uri`, `form-action`,
//! `object-src` or `frame-ancestors`. None of them falls back to
//! `default-src` in a useful way: injected markup could have re-based the
//! page's relative script URLs, posted a form to another origin or embedded
//! a plugin. Nothing injects markup today (see the sink checks below); the
//! policy now refuses those even if something one day does.

use pady_security::read;

fn csp() -> Vec<(String, Vec<String>)> {
    let conf: serde_json::Value = serde_json::from_str(&read("crates/pady-app/tauri.conf.json")).unwrap();
    let csp = conf["app"]["security"]["csp"].as_str().expect("a CSP is set").to_string();
    csp.split(';')
        .filter_map(|d| {
            let mut words = d.split_whitespace().map(str::to_string);
            Some((words.next()?, words.collect()))
        })
        .collect()
}

fn directive(name: &str) -> Option<Vec<String>> {
    csp().into_iter().find(|(n, _)| n == name).map(|(_, sources)| sources)
}

#[test]
fn the_policy_closes_what_default_src_does_not_cover() {
    for name in ["object-src", "base-uri", "form-action", "frame-ancestors"] {
        assert_eq!(directive(name), Some(vec!["'none'".to_string()]), "{name} must be 'none'");
    }
}

#[test]
fn scripts_come_from_the_app_only() {
    let scripts = directive("script-src").or_else(|| directive("default-src")).expect("a script policy");
    for source in &scripts {
        assert!(
            matches!(source.as_str(), "'self'" | "ipc:" | "http://ipc.localhost"),
            "scripts may come from {source}"
        );
    }
    let all: Vec<String> = csp().into_iter().flat_map(|(_, s)| s).collect();
    for banned in ["'unsafe-eval'", "*", "https:", "http:", "data:", "blob:"] {
        // `data:` is allowed for images only.
        let allowed_here = |name: &str| banned == "data:" && name == "img-src";
        for (name, sources) in csp() {
            assert!(allowed_here(&name) || !sources.iter().any(|s| s == banned), "{name} allows {banned}");
        }
    }
    assert!(!all.is_empty());
}

#[test]
fn no_remote_page_and_no_other_window_can_call_commands() {
    let conf: serde_json::Value = serde_json::from_str(&read("crates/pady-app/tauri.conf.json")).unwrap();
    for window in conf["app"]["windows"].as_array().unwrap() {
        let url = window["url"].as_str().unwrap_or("index.html");
        assert!(!url.contains("://"), "{} loads a remote page", window["label"]);
    }
    assert!(conf["app"]["security"].get("dangerousDisableAssetCspModification").is_none());
    for file in ["default.json", "overlay.json"] {
        let capability: serde_json::Value =
            serde_json::from_str(&read(&format!("crates/pady-app/capabilities/{file}"))).unwrap();
        assert!(capability.get("remote").is_none(), "{file} lets remote pages in");
        let windows = capability["windows"].as_array().unwrap();
        assert!(windows.iter().all(|w| w.as_str().is_some_and(|w| !w.contains('*'))), "{file}: no wildcard windows");
    }
}

/// The app's scripts, without comments (so a word in a comment can't hide or
/// fake a finding).
fn scripts() -> Vec<(String, String)> {
    ["app.js", "model.js", "icons.js", "overlay.js"]
        .into_iter()
        .map(|name| {
            let text = read(&format!("crates/pady-app/ui/{name}"));
            let code: String = text
                .lines()
                .filter(|l| {
                    let l = l.trim_start();
                    !(l.starts_with("//") || l.starts_with('*') || l.starts_with("/*"))
                })
                .collect::<Vec<_>>()
                .join("\n");
            (name.to_string(), code)
        })
        .collect()
}

/// Text only becomes HTML in three known places, each fed by the app's own
/// drawings. Anything from outside (device, app and profile names, errors,
/// release notes) goes through `textContent` / `createTextNode`.
#[test]
fn text_from_outside_never_becomes_html() {
    for (name, code) in scripts() {
        for sink in
            ["insertAdjacentHTML", "outerHTML", "document.write", "eval(", "new Function", "srcdoc", "javascript:"]
        {
            assert!(!code.contains(sink), "{name} uses {sink}");
        }
        let sinks = code.matches(".innerHTML").count();
        let expected = match name.as_str() {
            "app.js" => 2,     // h()'s `html:` key and frag()
            "overlay.js" => 1, // the level pill's icon, from the built-in ICONS table
            _ => 0,
        };
        assert_eq!(sinks, expected, "{name} has {sinks} innerHTML sinks; review any new one, then update this count");
    }
    let (_, app) = scripts().into_iter().find(|(n, _)| n == "app.js").unwrap();
    // h()'s `html:` key exists but nothing uses it: no `html:` attribute anywhere.
    assert!(!app.contains("html:"), "something passes `html:` to h(): check what it holds");
    // frag() only ever gets the app's own SVG builders.
    let calls: Vec<&str> = app.match_indices("frag(").map(|(i, _)| app[i..].lines().next().unwrap_or("")).collect();
    assert_eq!(calls.len(), 5, "a new frag() call: check what it is given\n{calls:#?}");
    for call in &calls {
        let known = [
            "frag(html) {",
            "frag(icon(name, size))",
            "frag(illoSVG(kind, n, dir))",
            "frag(DONE_SVG)",
            "frag(heroSVG())",
        ];
        assert!(known.iter().any(|k| call.contains(k)), "frag() is given something new: {call}");
    }
}

#[test]
fn the_pages_have_no_inline_script() {
    for page in ["index.html", "overlay.html"] {
        let html = read(&format!("crates/pady-app/ui/{page}")).to_ascii_lowercase();
        for (i, _) in html.match_indices("<script") {
            let tag = &html[i..i + html[i..].find('>').unwrap()];
            assert!(tag.contains("src=\""), "{page} has an inline script: {tag}");
            assert!(!tag.contains("://"), "{page} loads a script from elsewhere: {tag}");
        }
        for handler in [" onclick=", " onload=", " onerror=", " onmouseover=", " onfocus="] {
            assert!(!html.contains(handler), "{page} has an inline {handler} handler");
        }
        assert!(
            !html.contains("<iframe")
                && !html.contains("<object")
                && !html.contains("<embed")
                && !html.contains("<base")
        );
    }
}
