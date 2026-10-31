//! The languages the translator offers, with the codes each engine wants.

/// A language Pady can translate into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Language {
    /// ISO 639-1 code, as stored in the config (`translate_to`).
    pub code: &'static str,
    /// English name, shown in Settings and the translation pop-up.
    pub name: &'static str,
    /// ISO 639-3 code, as language detection reports it.
    pub iso3: &'static str,
    /// DeepL's target code, if DeepL translates into it.
    pub deepl: Option<&'static str>,
}

const fn lang(code: &'static str, name: &'static str, iso3: &'static str, deepl: Option<&'static str>) -> Language {
    Language { code, name, iso3, deepl }
}

/// Sorted by name, which is how Settings lists them.
pub const LANGUAGES: &[Language] = &[
    lang("ar", "Arabic", "ara", Some("AR")),
    lang("bg", "Bulgarian", "bul", Some("BG")),
    lang("zh", "Chinese", "cmn", Some("ZH-HANS")),
    lang("hr", "Croatian", "hrv", None),
    lang("cs", "Czech", "ces", Some("CS")),
    lang("da", "Danish", "dan", Some("DA")),
    lang("nl", "Dutch", "nld", Some("NL")),
    lang("en", "English", "eng", Some("EN-US")),
    lang("et", "Estonian", "est", Some("ET")),
    lang("fi", "Finnish", "fin", Some("FI")),
    lang("fr", "French", "fra", Some("FR")),
    lang("de", "German", "deu", Some("DE")),
    lang("el", "Greek", "ell", Some("EL")),
    lang("he", "Hebrew", "heb", Some("HE")),
    lang("hi", "Hindi", "hin", None),
    lang("hu", "Hungarian", "hun", Some("HU")),
    lang("id", "Indonesian", "ind", Some("ID")),
    lang("it", "Italian", "ita", Some("IT")),
    lang("ja", "Japanese", "jpn", Some("JA")),
    lang("ko", "Korean", "kor", Some("KO")),
    lang("lv", "Latvian", "lav", Some("LV")),
    lang("lt", "Lithuanian", "lit", Some("LT")),
    lang("nb", "Norwegian", "nob", Some("NB")),
    lang("fa", "Persian", "pes", None),
    lang("pl", "Polish", "pol", Some("PL")),
    lang("pt", "Portuguese", "por", Some("PT-BR")),
    lang("ro", "Romanian", "ron", Some("RO")),
    lang("ru", "Russian", "rus", Some("RU")),
    lang("sr", "Serbian", "srp", None),
    lang("sk", "Slovak", "slk", Some("SK")),
    lang("sl", "Slovenian", "slv", Some("SL")),
    lang("es", "Spanish", "spa", Some("ES")),
    lang("sv", "Swedish", "swe", Some("SV")),
    lang("th", "Thai", "tha", Some("TH")),
    lang("tr", "Turkish", "tur", Some("TR")),
    lang("uk", "Ukrainian", "ukr", Some("UK")),
    lang("ur", "Urdu", "urd", None),
    lang("vi", "Vietnamese", "vie", Some("VI")),
];

/// The language with this ISO 639-1 code (any case).
pub fn by_code(code: &str) -> Option<&'static Language> {
    let code = code.trim();
    LANGUAGES.iter().find(|l| l.code.eq_ignore_ascii_case(code))
}

/// The language with this ISO 639-3 code, as detection reports it.
pub fn by_iso3(iso3: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|l| l.iso3.eq_ignore_ascii_case(iso3))
}

/// The language DeepL reports as the source (`EN`, `PT`, `ZH`, `NB`...).
pub fn by_deepl_source(code: &str) -> Option<&'static Language> {
    let base = code.split('-').next().unwrap_or(code);
    by_code(base)
}

/// The name for a code, or the code itself for one Pady doesn't list.
pub fn name(code: &str) -> String {
    by_code(code).map_or_else(|| code.to_uppercase(), |l| l.name.to_string())
}

/// Written right to left.
pub fn is_rtl(code: &str) -> bool {
    matches!(code, "ar" | "he" | "fa" | "ur")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_names_sorted() {
        for (i, a) in LANGUAGES.iter().enumerate() {
            for b in &LANGUAGES[i + 1..] {
                assert_ne!(a.code, b.code);
                assert_ne!(a.iso3, b.iso3);
                assert!(a.name < b.name, "{} before {}", a.name, b.name);
            }
        }
    }

    #[test]
    fn lookups() {
        assert_eq!(by_code(" EN ").unwrap().name, "English");
        assert_eq!(by_iso3("ara").unwrap().code, "ar");
        assert_eq!(by_deepl_source("PT").unwrap().code, "pt");
        assert_eq!(by_deepl_source("EN-GB").unwrap().code, "en");
        assert_eq!(name("xx"), "XX");
        assert!(is_rtl("ar") && !is_rtl("en"));
        assert_eq!(by_code("hi").unwrap().deepl, None);
    }
}
