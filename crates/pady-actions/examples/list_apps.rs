//! Lists the installed apps the "Choose an app" menu offers, and how long
//! that took:
//!
//!     cargo run -p pady-actions --release --example list_apps [folder-for-icons]

use std::time::Instant;

fn main() {
    let start = Instant::now();
    let apps = pady_actions::installed_apps();
    let took = start.elapsed();
    let icons = apps.iter().filter(|a| a.icon.is_some()).count();
    let bytes: usize =
        apps.iter().map(|a| a.name.len() + a.program.len() + a.icon.as_ref().map_or(0, String::len)).sum();
    for app in &apps {
        println!("{:<38} {}", app.name, app.program);
    }
    println!("\n{} apps ({icons} with an icon), {} KB to send, listed in {took:?}", apps.len(), bytes / 1024);
    if let Some(folder) = std::env::args().nth(1) {
        std::fs::create_dir_all(&folder).expect("create the folder");
        for app in apps.iter().filter(|a| a.icon.is_some()).take(12) {
            let name: String = app.name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            let data = app.icon.as_ref().unwrap().trim_start_matches("data:image/png;base64,");
            std::fs::write(format!("{folder}/{name}.b64"), data).expect("write an icon");
        }
    }
}
