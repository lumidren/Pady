//! Self-update through tauri-plugin-updater. A quiet check runs shortly after
//! start-up and then twice a day (unless turned off in Settings); a new
//! version shows up in the tray menu and as a toast in the window. Installing
//! downloads it, verifies its signature against the public key in
//! tauri.conf.json, runs the installer and restarts Pady.

use crate::{lock, Pady};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::MenuItem;
use tauri::{AppHandle, Emitter, Manager, Wry};
use tauri_plugin_updater::UpdaterExt;

const FIRST_CHECK: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(12 * 60 * 60);
pub const TRAY_LABEL: &str = "Check for updates";
/// What a person is told when a check fails; the details go to the log.
const UNREACHABLE: &str = "Pady couldn't reach GitHub. Check your connection and try again.";

/// GitHub answered, but has no release to compare with: there is simply
/// nothing newer, which is not an error to bother anyone with.
fn nothing_published(error: &tauri_plugin_updater::Error) -> bool {
    matches!(error, tauri_plugin_updater::Error::ReleaseNotFound)
}

#[derive(Clone, Debug, Serialize)]
pub struct Available {
    pub version: String,
    pub notes: Option<String>,
    /// False for .deb/.rpm installs, which update through the package manager.
    pub can_install: bool,
}

#[derive(Default)]
pub struct Updates {
    available: Mutex<Option<Available>>,
    pub tray_item: Mutex<Option<MenuItem<Wry>>>,
    installing: AtomicBool,
}

/// The Windows installers and the AppImage can replace themselves; .deb and
/// .rpm installs belong to the package manager.
fn can_install() -> bool {
    cfg!(windows) || std::env::var_os("APPIMAGE").is_some()
}

fn set_tray_text(app: &AppHandle, text: &str) {
    if let Some(item) = &*lock(&app.state::<Updates>().tray_item) {
        let _ = item.set_text(text);
    }
}

/// A beta (a version like `1.0.0-beta`) is installed by hand and never
/// updated: the stable release of the same number would replace it.
pub fn is_beta(app: &AppHandle) -> bool {
    !app.package_info().version.pre.is_empty()
}

/// Ask GitHub for a newer release. A new one is announced in the tray and to
/// the window (`pady://update`).
pub async fn check(app: &AppHandle) -> Result<Option<Available>, String> {
    if is_beta(app) {
        return Ok(None);
    }
    let update = match app.updater().map_err(|e| e.to_string())?.check().await {
        Ok(update) => update,
        Err(e) if nothing_published(&e) => {
            log::info!("update check: no release is published, so there is nothing newer");
            None
        }
        Err(e) => {
            log::info!("update check: {e}");
            return Err(UNREACHABLE.into());
        }
    };
    let found =
        update.map(|u| Available { version: u.version.clone(), notes: u.body.clone(), can_install: can_install() });
    *lock(&app.state::<Updates>().available) = found.clone();
    if let Some(update) = &found {
        log::info!("Pady {} is available", update.version);
        set_tray_text(app, &format!("Update to Pady {}", update.version));
        let _ = app.emit("pady://update", update);
    }
    Ok(found)
}

/// Download, verify and install the latest release, then restart. On
/// Windows the installer closes Pady itself.
pub async fn install(app: AppHandle) -> Result<(), String> {
    if !can_install() {
        return Err(
            "Update Pady with your package manager, or download the new version from lumidren.github.io/Pady.".into()
        );
    }
    let updates = app.state::<Updates>();
    if updates.installing.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    set_tray_text(&app, "Downloading update…");
    let result = async {
        let update = app
            .updater()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Pady is already up to date.".to_string())?;
        log::info!("installing Pady {}", update.version);
        // Never leave the mouse button held across the restart.
        app.state::<Pady>().engine.release_mouse(Duration::from_millis(300));
        update.download_and_install(|_, _| {}, || {}).await.map_err(|e| e.to_string())
    }
    .await;
    match result {
        Ok(()) => app.restart(),
        Err(e) => {
            log::warn!("update failed: {e}");
            updates.installing.store(false, Ordering::SeqCst);
            set_tray_text(&app, TRAY_LABEL);
            Err(e)
        }
    }
}

/// The tray item: install a known update, otherwise check and say so.
pub fn tray_clicked(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let known = lock(&app.state::<Updates>().available).clone();
        match known {
            Some(update) if update.can_install => {
                let _ = install(app).await;
            }
            Some(_) => crate::show_main(&app),
            None => {
                set_tray_text(&app, "Checking for updates…");
                match check(&app).await {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        say_in_tray(app, "No update available");
                    }
                    Err(e) => {
                        log::warn!("update check failed: {e}");
                        say_in_tray(app, "Couldn't check for updates");
                    }
                }
            }
        }
    });
}

/// The tray item says `text` for a few seconds, then offers the check again.
fn say_in_tray(app: AppHandle, text: &str) {
    set_tray_text(&app, text);
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(Duration::from_secs(5));
        // An update found meanwhile keeps its own text.
        if lock(&app.state::<Updates>().available).is_none() {
            set_tray_text(&app, TRAY_LABEL);
        }
    });
}

/// Background checks: once shortly after start-up, then twice a day.
pub fn spawn_checks(app: AppHandle) {
    if is_beta(&app) {
        log::info!("a beta: no update checks");
        return;
    }
    std::thread::Builder::new()
        .name("pady-updates".into())
        .spawn(move || {
            std::thread::sleep(FIRST_CHECK);
            loop {
                if app.state::<Pady>().config.get().settings.check_updates {
                    if let Err(e) = tauri::async_runtime::block_on(check(&app)) {
                        log::info!("update check failed: {e}");
                    }
                }
                std::thread::sleep(CHECK_EVERY);
            }
        })
        .expect("spawn update thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri_plugin_updater::Error;

    #[test]
    fn no_published_release_means_no_update_not_an_error() {
        assert!(nothing_published(&Error::ReleaseNotFound));
        assert!(!nothing_published(&Error::EmptyEndpoints), "a real fault still is one");
        assert!(!UNREACHABLE.contains("JSON"), "people are told in plain words");
    }
}
