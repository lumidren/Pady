//! Translating text online: MyMemory (free, no account) or DeepL (the
//! user's own key). The pop-up that shows the result is in `popup.rs`.

use pady_core::languages::{self, Language};
use serde::Serialize;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;

/// Longer selections are refused rather than cut: half a translation reads
/// like a whole one.
pub const MAX_CHARS: usize = 5_000;
/// MyMemory takes up to 500 bytes per request.
const FREE_CHUNK_BYTES: usize = 480;
const MYMEMORY: &str = "https://api.mymemory.translated.net/get";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Translation {
    pub text: String,
    /// The detected source language (ISO 639-1), if known.
    pub from: Option<String>,
    pub from_name: Option<String>,
    pub to: String,
    pub to_name: String,
    /// Who translated it, for the pop-up's footer.
    pub engine: &'static str,
    /// The text already was in the target language, so nothing changed.
    pub unchanged: bool,
}

/// What went wrong, in words for the pop-up.
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    TooLong(usize),
    Offline,
    FreeQuota,
    DeeplKey,
    DeeplQuota,
    Busy,
    Other(String),
}

impl Problem {
    pub fn message(&self) -> String {
        match self {
            Problem::TooLong(n) => {
                format!("That's {n} characters. Select up to {MAX_CHARS} at a time.")
            }
            Problem::Offline => "Couldn't reach the translation service. Check your internet connection.".into(),
            Problem::FreeQuota => {
                "The free translator's daily limit is used up. It resets tomorrow, or add a DeepL key in Settings."
                    .into()
            }
            Problem::DeeplKey => "DeepL didn't accept the key. Check it in Settings › Translator.".into(),
            Problem::DeeplQuota => "Your DeepL quota for this month is used up.".into(),
            Problem::Busy => "The translation service is busy. Try again in a moment.".into(),
            Problem::Other(e) => e.clone(),
        }
    }

    /// Worth a Retry button.
    pub fn retryable(&self) -> bool {
        matches!(self, Problem::Offline | Problem::Busy | Problem::Other(_))
    }
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        reqwest::Client::builder()
            .user_agent(concat!("Pady/", env!("CARGO_PKG_VERSION"), " (+https://github.com/lumidren/Pady)"))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .build()
            .unwrap_or_default()
    })
}

fn net(e: reqwest::Error) -> Problem {
    if e.is_connect() || e.is_timeout() {
        Problem::Offline
    } else {
        Problem::Other(format!("The translation failed: {e}"))
    }
}

/// The language `text` is written in, as an ISO 639-1 code, if it can tell.
pub fn detect(text: &str) -> Option<&'static str> {
    let info = whatlang::detect(text)?;
    let iso3 = info.lang().code();
    if let Some(lang) = languages::by_iso3(iso3) {
        return Some(lang.code);
    }
    // Languages Pady doesn't translate into, but can still translate from.
    Some(match iso3 {
        "afr" => "af",
        "bel" => "be",
        "ben" => "bn",
        "cat" => "ca",
        "epo" => "eo",
        "guj" => "gu",
        "hye" => "hy",
        "kat" => "ka",
        "kan" => "kn",
        "lat" => "la",
        "mal" => "ml",
        "mar" => "mr",
        "mkd" => "mk",
        "nep" => "ne",
        "pan" => "pa",
        "tam" => "ta",
        "tel" => "te",
        "tgl" => "tl",
        "aze" => "az",
        "uzb" => "uz",
        _ => return None,
    })
}

/// Translate `text` into `to` with the engine the settings choose. DeepL
/// falls back to the free service for languages it doesn't offer.
pub async fn translate(
    text: &str,
    to: &'static Language,
    engine: &str,
    deepl_key: &str,
) -> Result<Translation, Problem> {
    let text = text.trim();
    let chars = text.chars().count();
    if chars > MAX_CHARS {
        return Err(Problem::TooLong(chars));
    }
    match (engine, to.deepl) {
        ("deepl", Some(target)) if !deepl_key.is_empty() => deepl(text, to, target, deepl_key).await,
        _ => free(text, to).await,
    }
}

fn done(text: String, from: Option<&str>, to: &Language, engine: &'static str, unchanged: bool) -> Translation {
    Translation {
        text,
        from: from.map(str::to_string),
        from_name: from.map(languages::name),
        to: to.code.into(),
        to_name: to.name.into(),
        engine,
        unchanged,
    }
}

// ---------------------------------------------------------------------------
// MyMemory

async fn free(text: &str, to: &Language) -> Result<Translation, Problem> {
    let from = detect(text);
    if from == Some(to.code) {
        return Ok(done(text.to_string(), from, to, "MyMemory", true));
    }
    let pair = format!("{}|{}", from.unwrap_or("autodetect"), to.code);
    let mut out = String::with_capacity(text.len());
    for (piece, gap) in chunks(text, FREE_CHUNK_BYTES) {
        let response =
            client().get(MYMEMORY).query(&[("q", piece), ("langpair", pair.as_str())]).send().await.map_err(net)?;
        let status = response.status().as_u16();
        let body: Value = response.json().await.map_err(net)?;
        out.push_str(&mymemory_text(status, &body)?);
        out.push_str(gap);
    }
    Ok(done(out.trim_end().to_string(), from, to, "MyMemory", false))
}

/// The translation in a MyMemory reply, or why there isn't one.
fn mymemory_text(http: u16, body: &Value) -> Result<String, Problem> {
    // responseStatus is a number, or sometimes the number as a string.
    let status = match &body["responseStatus"] {
        Value::Number(n) => n.as_u64().unwrap_or(0) as u16,
        Value::String(s) => s.parse().unwrap_or(0),
        _ => http,
    };
    let details = body["responseDetails"].as_str().unwrap_or_default();
    let text = body["responseData"]["translatedText"].as_str().unwrap_or_default();
    if status == 429 || details.contains("USED ALL AVAILABLE FREE TRANSLATIONS") || text.contains("MYMEMORY WARNING") {
        return Err(Problem::FreeQuota);
    }
    if status >= 500 || http >= 500 {
        return Err(Problem::Busy);
    }
    if status != 200 || text.is_empty() {
        let why = if details.is_empty() { format!("status {status}") } else { details.to_lowercase() };
        return Err(Problem::Other(format!("The free translator couldn't translate this ({why}).")));
    }
    Ok(unescape(text))
}

/// MyMemory sometimes returns HTML entities (`&#39;`, `&quot;`).
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Split `text` into pieces of at most `max` bytes, at paragraph, sentence
/// or word ends where possible. Each piece comes with the whitespace that
/// followed it, so line breaks survive.
fn chunks(text: &str, max: usize) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let cut = if rest.len() <= max { rest.len() } else { split_point(rest, max) };
        let (piece, tail) = rest.split_at(cut);
        let gap_len = tail.len() - tail.trim_start().len();
        let (gap, next) = tail.split_at(gap_len);
        let piece = piece.trim_end();
        if !piece.is_empty() {
            out.push((piece, if next.is_empty() { "" } else { gap_kind(gap) }));
        }
        rest = next;
    }
    out
}

/// The separator to put back after a piece: keep paragraph and line breaks.
fn gap_kind(gap: &str) -> &'static str {
    match gap.matches('\n').count() {
        0 => " ",
        1 => "\n",
        _ => "\n\n",
    }
}

fn split_point(s: &str, max: usize) -> usize {
    let mut limit = max;
    while !s.is_char_boundary(limit) {
        limit -= 1;
    }
    let head = &s[..limit];
    // Cut after the punctuation but before the whitespace, which becomes
    // the gap that's put back between the translated pieces.
    let cut = |i: usize, sep: &str| i + sep.trim_end().len();
    for seps in [&["\n\n"][..], &["\n"], &[". ", "! ", "? ", "。", "؟ "], &[", ", "; ", "، "], &[" "]] {
        if let Some((i, sep)) = seps.iter().filter_map(|sep| head.rfind(sep).map(|i| (i, *sep))).max_by_key(|(i, _)| *i)
        {
            if cut(i, sep) > 0 {
                return cut(i, sep);
            }
        }
    }
    limit
}

// ---------------------------------------------------------------------------
// DeepL

fn deepl_endpoint(key: &str) -> &'static str {
    // Free-plan keys end in ":fx" and have their own server.
    if key.ends_with(":fx") {
        "https://api-free.deepl.com/v2/translate"
    } else {
        "https://api.deepl.com/v2/translate"
    }
}

async fn deepl(text: &str, to: &Language, target: &str, key: &str) -> Result<Translation, Problem> {
    let body = serde_json::json!({ "text": [text], "target_lang": target });
    let response = client()
        .post(deepl_endpoint(key))
        .header("Authorization", format!("DeepL-Auth-Key {key}"))
        .json(&body)
        .send()
        .await
        .map_err(net)?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err(Problem::DeeplKey),
        456 => return Err(Problem::DeeplQuota),
        429 | 500.. => return Err(Problem::Busy),
        other => return Err(Problem::Other(format!("DeepL couldn't translate this (status {other})."))),
    }
    let reply: Value = response.json().await.map_err(net)?;
    let (translated, source) = deepl_text(&reply).ok_or_else(|| Problem::Other("DeepL sent an empty reply.".into()))?;
    let from = source.and_then(languages::by_deepl_source).map(|l| l.code);
    Ok(done(translated, from, to, "DeepL", from == Some(to.code) && translated_same(text, &reply)))
}

fn deepl_text(reply: &Value) -> Option<(String, Option<&str>)> {
    let first = reply["translations"].get(0)?;
    let text = first["text"].as_str()?.to_string();
    Some((text, first["detected_source_language"].as_str()))
}

fn translated_same(original: &str, reply: &Value) -> bool {
    deepl_text(reply).is_some_and(|(t, _)| t.trim() == original.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chunks_keep_sentences_and_line_breaks() {
        let text = "One sentence here. Another one follows.\nA new line.\n\nA paragraph.";
        let parts = chunks(text, 25);
        assert!(parts.iter().all(|(p, _)| p.len() <= 25), "{parts:?}");
        let rebuilt: String = parts.iter().map(|(p, g)| format!("{p}{g}")).collect();
        assert_eq!(rebuilt, "One sentence here. Another one follows.\nA new line.\n\nA paragraph.");
        assert_eq!(parts[0].0, "One sentence here.");
    }

    #[test]
    fn chunks_never_split_a_character() {
        let text = "مرحبا بكم في هذا الاختبار الطويل جدا ".repeat(40);
        for (piece, _) in chunks(&text, FREE_CHUNK_BYTES) {
            assert!(piece.len() <= FREE_CHUNK_BYTES);
        }
        let word = "ä".repeat(400); // no spaces at all
        let parts = chunks(&word, 101);
        assert!(parts.iter().all(|(p, _)| p.len() <= 101));
        assert_eq!(parts.iter().map(|(p, _)| *p).collect::<String>(), word);
    }

    #[test]
    fn reads_mymemory_replies() {
        let ok = json!({ "responseData": { "translatedText": "Hello, how&#39;s it going? &quot;fine&quot; &amp; you" }, "responseStatus": 200 });
        assert_eq!(mymemory_text(200, &ok).unwrap(), "Hello, how's it going? \"fine\" & you");
        let quota = json!({ "responseData": { "translatedText": "MYMEMORY WARNING: YOU USED ALL AVAILABLE FREE TRANSLATIONS FOR TODAY." }, "responseStatus": 429 });
        assert_eq!(mymemory_text(200, &quota), Err(Problem::FreeQuota));
        let bad = json!({ "responseData": { "translatedText": "" }, "responseStatus": "403", "responseDetails": "INVALID LANGUAGE PAIR" });
        assert!(matches!(mymemory_text(200, &bad), Err(Problem::Other(m)) if m.contains("invalid language pair")));
        assert_eq!(mymemory_text(503, &json!({})), Err(Problem::Busy));
    }

    #[test]
    fn unescape_leaves_plain_ampersands() {
        assert_eq!(unescape("Fish & chips &unknown; &#x41;&#66;"), "Fish & chips &unknown; AB");
        assert_eq!(unescape("a & b"), "a & b");
    }

    #[test]
    fn reads_deepl_replies() {
        let reply = json!({ "translations": [{ "detected_source_language": "ES", "text": "Hello" }] });
        assert_eq!(deepl_text(&reply), Some(("Hello".into(), Some("ES"))));
        assert_eq!(deepl_text(&json!({ "translations": [] })), None);
        assert_eq!(deepl_endpoint("abc:fx"), "https://api-free.deepl.com/v2/translate");
        assert_eq!(deepl_endpoint("abc"), "https://api.deepl.com/v2/translate");
    }

    #[test]
    fn detects_common_languages() {
        assert_eq!(detect("Hola, ¿cómo estás? Espero que todo vaya bien en el trabajo."), Some("es"));
        assert_eq!(detect("Bonjour, comment allez-vous aujourd'hui ? J'espère que tout va bien."), Some("fr"));
        assert_eq!(detect("مرحبا، كيف حالك اليوم؟ أتمنى أن يكون كل شيء على ما يرام."), Some("ar"));
        assert_eq!(detect("Привет, как дела? Надеюсь, у тебя всё хорошо."), Some("ru"));
    }

    #[test]
    fn too_long_is_refused_before_any_request() {
        let to = languages::by_code("en").unwrap();
        let text = "a".repeat(MAX_CHARS + 1);
        let result = tauri::async_runtime::block_on(translate(&text, to, "free", ""));
        assert_eq!(result, Err(Problem::TooLong(MAX_CHARS + 1)));
    }

    /// Talks to the real services: `cargo test -p pady-app live_ -- --ignored`.
    #[test]
    #[ignore]
    fn live_free_translation() {
        let to = languages::by_code("en").unwrap();
        let t =
            tauri::async_runtime::block_on(translate("Hola, ¿cómo estás? Hoy hace muy buen tiempo.", to, "free", ""))
                .unwrap();
        println!("{t:?}");
        assert_eq!(t.from.as_deref(), Some("es"));
        assert!(t.text.to_lowercase().contains("how are you"), "{}", t.text);
    }
}
