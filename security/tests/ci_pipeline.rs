//! PADY-08: the build and release pipeline trusted more than it had to.
//!
//! * Actions were pinned to tags (`@v4`, `@stable`). A tag can be moved: who
//!   controls a third-party action's repository controls what runs in the
//!   release job next time, with the repository's write token.
//! * The updater signing key was an environment variable of the whole
//!   release job, so every step could read it: the cache action, the npm
//!   package that is the Tauri CLI, the signing service's action.
//! * The CI workflow set no `permissions`, leaving the token's reach to the
//!   repository's default.
//!
//! These tests keep the workflows the way the fix left them.

use pady_security::{read, repo};

fn workflows() -> Vec<(String, String)> {
    let dir = repo().join(".github").join("workflows");
    let mut found: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "yml" || x == "yaml"))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read_to_string(e.path()).unwrap()))
        .collect();
    found.sort();
    assert!(found.len() >= 3, "ci, pages and release workflows at least");
    found
}

/// Lines that are not comments, with their indentation.
fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines().filter(|l| !l.trim_start().starts_with('#')).map(|l| (l.len() - l.trim_start().len(), l.trim()))
}

#[test]
fn every_action_is_pinned_to_a_full_commit() {
    for (name, text) in workflows() {
        for (_, line) in lines(&text) {
            let Some(at) = line.find("uses:") else { continue };
            let used = line[at + 5..].split('#').next().unwrap().trim();
            let (action, reference) =
                used.rsplit_once('@').unwrap_or_else(|| panic!("{name}: `{used}` has no version"));
            assert!(
                reference.len() == 40 && reference.chars().all(|c| c.is_ascii_hexdigit()),
                "{name}: {action}@{reference} is a tag or branch, which can be moved; pin the commit"
            );
            assert!(line.contains(" # "), "{name}: say which version {action}@{reference} is, in a comment");
        }
    }
}

#[test]
fn every_workflow_limits_its_token() {
    for (name, text) in workflows() {
        let top_level: Vec<&str> = lines(&text).filter(|(indent, _)| *indent == 0).map(|(_, l)| l).collect();
        assert!(top_level.contains(&"permissions:"), "{name} sets no top-level permissions");
        assert!(!text.contains("write-all"), "{name} gives the token everything");
    }
    // Only the release workflow may write to the repository's contents.
    for (name, text) in workflows() {
        if name != "release.yml" {
            assert!(!text.contains("contents: write"), "{name} can write to the repository");
        }
    }
}

#[test]
fn pull_requests_from_forks_never_run_with_secrets() {
    for (name, text) in workflows() {
        assert!(!text.contains("pull_request_target"), "{name} runs fork code with the base repository's secrets");
        assert!(!text.contains("workflow_run"), "{name} is triggered by another workflow's (possibly fork) run");
    }
    // The workflow that holds the signing secrets is started by a tag or by hand only.
    let text = read(".github/workflows/release.yml");
    let start = text.find("on:").unwrap();
    let on = &text[start..start + text[start..].find("permissions:").unwrap()];
    assert!(on.contains("tags:") && on.contains("workflow_dispatch"), "{on}");
    assert!(!on.contains("pull_request") && !on.contains("branches:"), "{on}");
}

#[test]
fn secrets_reach_only_the_steps_that_need_them() {
    let text = read(".github/workflows/release.yml");
    for (indent, line) in lines(&text) {
        if !line.contains("secrets.") {
            continue;
        }
        // At job level (6 spaces) only "is this secret set?" may appear; a
        // secret's value belongs in one step's own `env:` or `with:` (10+).
        let only_checks_presence = line.contains("!= ''");
        assert!(indent >= 10 || only_checks_presence, "a secret's value is visible to the whole job: {line}");
    }
    // The updater key: named by exactly the two steps that sign.
    assert_eq!(text.matches("TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}").count(), 2);
    // No workflow prints or forwards secrets wholesale.
    for (name, text) in workflows() {
        assert!(!text.contains("toJSON(secrets)") && !text.contains("toJson(secrets)"), "{name}");
        assert!(!text.contains("secrets: inherit"), "{name}");
    }
}

#[test]
fn checkouts_do_not_leave_the_token_behind() {
    for (name, text) in workflows() {
        let checkouts = text.matches("uses: actions/checkout@").count();
        let cleared = text.matches("persist-credentials: false").count();
        assert_eq!(checkouts, cleared, "{name}: a checkout keeps the token in .git/config for later steps");
    }
}

#[test]
fn the_tauri_cli_and_the_lockfile_are_pinned() {
    let release = read(".github/workflows/release.yml");
    let cli = release.lines().find(|l| l.trim_start().starts_with("TAURI_CLI:")).expect("TAURI_CLI");
    let version = cli.split('@').next_back().unwrap().trim_matches(|c| c == '"' || c == ' ');
    assert!(
        version.split('.').count() == 3 && version.split('.').all(|p| p.parse::<u32>().is_ok()),
        "the Tauri CLI must be an exact version, not `{version}`"
    );
    assert!(release.contains("cargo build --release --locked"), "pady-cli builds from the lockfile");
    assert!(repo().join("Cargo.lock").is_file(), "Cargo.lock is committed");
    let ignore = std::fs::read_to_string(repo().join(".gitignore")).unwrap_or_default();
    assert!(!ignore.lines().any(|l| l.trim() == "Cargo.lock"), "Cargo.lock must not be ignored");
}
