//! Reads the selected text in the app in front after a short delay, then
//! checks the clipboard is back as it was:
//!   cargo run -p pady-actions --example selection -- 3
fn main() {
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(3);
    println!("select some text in another app; reading it in {secs} s...");
    std::thread::sleep(std::time::Duration::from_secs(secs));
    let started = std::time::Instant::now();
    match pady_actions::selected_text() {
        Ok(Some(text)) => println!("selected ({:?}): {text:?}", started.elapsed()),
        Ok(None) => println!("nothing selected ({:?})", started.elapsed()),
        Err(e) => println!("failed: {e}"),
    }
}
