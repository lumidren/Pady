//! The self-updater downloads and runs an installer, so its settings decide
//! who can run code on every user's machine. Nothing was wrong here; these
//! tests keep it that way.
//!
//! What holds (checked below, and in tauri-plugin-updater's code):
//! * the update manifest and the installer come over HTTPS from GitHub only;
//! * the installer runs only if its signature verifies against the public
//!   key compiled into Pady, so neither a tampered download nor someone with
//!   write access to the repository (but not the key) can ship an update;
//! * only a version *newer* than the running one is offered (the plugin's
//!   default comparison), so an old signed installer can't be replayed as a
//!   downgrade;
//! * betas never update themselves.

use pady_security::read;

fn conf() -> serde_json::Value {
    serde_json::from_str(&read("crates/pady-app/tauri.conf.json")).unwrap()
}

#[test]
fn updates_come_over_https_from_the_projects_own_releases() {
    let updater = &conf()["plugins"]["updater"];
    let endpoints = updater["endpoints"].as_array().expect("endpoints");
    assert!(!endpoints.is_empty());
    for endpoint in endpoints {
        let url = endpoint.as_str().unwrap();
        assert!(url.starts_with("https://github.com/lumidren/Pady/releases/"), "{url}");
    }
    assert!(updater.get("dangerousInsecureTransportProtocol").is_none(), "plain HTTP must stay off");
    assert!(updater.get("dangerousAcceptInvalidCerts").is_none());
    assert!(updater.get("dangerousAcceptInvalidHostnames").is_none());
}

#[test]
fn a_signing_key_is_compiled_in() {
    let key = conf()["plugins"]["updater"]["pubkey"].as_str().expect("pubkey").to_string();
    assert!(key.len() > 100, "the updater public key is missing or cut short");
    // A minisign public key file, base64 encoded: it starts with this comment.
    assert!(key.starts_with("dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6"), "not a minisign public key");
}

#[test]
fn nothing_overrides_the_plugins_checks() {
    let update = read("crates/pady-app/src/update.rs");
    // A custom comparison could offer older versions; the default only offers newer ones.
    for override_ in ["version_comparator", "dangerous", "pubkey(", "endpoints(", "allow_downgrades"] {
        assert!(!update.contains(override_), "update.rs uses `{override_}`: check what it weakens");
    }
    let main = read("crates/pady-app/src/main.rs");
    assert!(main.contains("tauri_plugin_updater::Builder::new().build()"), "the updater is built with its defaults");
    assert!(update.contains("download_and_install"), "the plugin both verifies and installs");
    assert!(update.contains("is_beta"), "betas don't update themselves");
}

#[test]
fn the_updater_talks_tls_with_a_vetted_stack() {
    let manifest = read("crates/pady-app/Cargo.toml");
    let line = manifest.lines().find(|l| l.starts_with("tauri-plugin-updater")).expect("the updater dependency");
    assert!(line.contains("rustls-tls"), "{line}");
    assert!(!line.contains("native-tls-vendored"), "{line}");
}
