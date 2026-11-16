//! PADY-09: the website built HTML from GitHub's API answer.
//!
//! `site/site.js` asks GitHub for the latest release and wrote its asset
//! names and download links into the page with `innerHTML`. Whoever can
//! publish a release asset (or anyone, if that API answer is ever wrong or
//! spoofed) could have put markup, and with it script, into the project's
//! own site. The links are now built as elements and must point at the
//! project's releases.

use pady_security::read;

fn code(text: &str) -> String {
    text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

#[test]
fn release_data_never_becomes_html() {
    let js = code(&read("site/site.js"));
    assert!(!js.contains("insertAdjacentHTML"), "site.js inserts HTML");
    // The one innerHTML left builds a play button from the page's own markup.
    let sinks: Vec<&str> = js.match_indices("innerHTML").map(|(i, _)| js[..i].lines().last().unwrap_or("")).collect();
    assert_eq!(sinks.len(), 1, "{sinks:#?}");
    assert!(sinks[0].trim_start().starts_with("button."), "{sinks:#?}");
    for field in ["browser_download_url", "html_url", "asset.name", "tag_name"] {
        for (i, _) in js.match_indices(field) {
            let line = js[..i].lines().last().unwrap_or("").to_string() + js[i..].lines().next().unwrap_or("");
            assert!(!line.contains('`') || !line.contains('<'), "release data goes into markup: {line}");
        }
    }
}

#[test]
fn download_links_must_point_at_the_projects_releases() {
    let js = code(&read("site/site.js"));
    assert!(js.contains("https://github.com/lumidren/Pady/releases/"), "links are checked against the releases URL");
    assert!(js.contains("safeLink("), "every link from the API goes through the check");
    // Each place a URL from the API is used goes through safeLink.
    for field in ["browser_download_url", "html_url"] {
        for (i, _) in js.match_indices(field) {
            let line = js[..i].lines().last().unwrap_or("").to_string() + js[i..].lines().next().unwrap_or("");
            assert!(line.contains("safeLink("), "{field} is used without the check: {line}");
        }
    }
}

#[test]
fn the_site_loads_no_script_from_elsewhere() {
    for page in ["index.html", "features.html", "video.html"] {
        let html = read(&format!("site/{page}"));
        for (i, _) in html.match_indices("<script") {
            let tag = &html[i..i + html[i..].find('>').unwrap()];
            assert!(!tag.contains("://"), "{page} loads a script from another site: {tag}");
        }
        for (i, _) in html.match_indices("target=\"_blank\"") {
            let start = html[..i].rfind('<').unwrap();
            let tag = &html[start..start + html[start..].find('>').unwrap()];
            assert!(tag.contains("noopener"), "{page}: a new-tab link can reach back into the page: {tag}");
        }
    }
}
