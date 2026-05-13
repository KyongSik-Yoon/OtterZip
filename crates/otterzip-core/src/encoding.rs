//! Filename / comment encoding decoder for ZIP archives.
//!
//! The ZIP specification (APPNOTE.TXT §4.4.4) requires either CP437
//! or UTF-8 for entry names. Real-world archives produced before the
//! UTF-8 flag (GP bit 11) was widely adopted use the local MBCS — in
//! Korea that's almost always CP949 (also known as UHC / MS949, a
//! superset of EUC-KR that adds the full 11,172 modern Hangul
//! syllables). Decoding those bytes as CP437 produces 모지바케 / mojibake
//! ("주문서.pdf" → "占쏙옙占쏙옙占쏙옙.pdf").
//!
//! Bandizip / 알집 / 7-Zip-with-mcp all support some flavour of this;
//! the strategy that performs best on shared test corpora is a 3-tier
//! cascade, mirrored here:
//!
//!   1. **GP bit 11 set OR Info-ZIP `0x7075` extra field present** →
//!      bytes are guaranteed UTF-8 (or the 0x7075 payload is). Trust.
//!   2. **`std::str::from_utf8` validates** → tentative UTF-8.
//!      Catches archives where bit 11 wasn't set but the bytes
//!      happen to be valid UTF-8 (common for ASCII-only filenames).
//!   3. Otherwise → **legacy MBCS dispatch**. We feed the whole
//!      central directory into `chardetng` (CJK-aware detector
//!      Firefox uses for legacy pages) and pick a codepage. If the
//!      detector returns a non-CJK encoding (windows-1252 etc.) we
//!      fall back to the OS locale (KR → CP949, JP → Shift_JIS, …)
//!      and finally to a user-configured override.
//!
//! Tier 1 + 2 are per-entry; tier 3 picks one codepage for the whole
//! archive because a single user-locale archive can't realistically
//! mix encodings. The detector decision is logged via `tracing` so
//! "why did this archive's names look right / wrong?" stays
//! diagnosable from the same log file the FFI subscriber writes.

use encoding_rs::{Encoding, BIG5, EUC_KR, GBK, SHIFT_JIS, UTF_8};

/// What the cascade decided. Held alongside the decoded archive in
/// case downstream code (UI, audit log, tests) wants to surface
/// "filenames were decoded as CP949 — was that right?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameEncoding {
    /// Bytes were valid UTF-8 (or 0x7075 extra field carried UTF-8).
    Utf8,
    /// Legacy MBCS: CP949 / Shift_JIS / GBK / Big5.
    Legacy(LegacyCodepage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyCodepage {
    Cp949,    // Korean — encoding_rs EUC_KR backs the full UHC map.
    ShiftJis, // Japanese
    Gbk,      // Simplified Chinese (also covers GB2312)
    Big5,     // Traditional Chinese
}

impl LegacyCodepage {
    pub fn encoding(self) -> &'static Encoding {
        match self {
            Self::Cp949 => EUC_KR,
            Self::ShiftJis => SHIFT_JIS,
            Self::Gbk => GBK,
            Self::Big5 => BIG5,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cp949 => "CP949",
            Self::ShiftJis => "Shift_JIS",
            Self::Gbk => "GBK",
            Self::Big5 => "Big5",
        }
    }
}

/// User override sourced from `Settings_LegacyCodepage`. `Auto` runs
/// the full detector cascade; the other variants short-circuit to a
/// fixed codepage so power users can pin behaviour for their corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyCodepageOverride {
    Auto,
    Utf8,
    Legacy(LegacyCodepage),
}

impl Default for LegacyCodepageOverride {
    fn default() -> Self {
        Self::Auto
    }
}

/// One name + the metadata needed for tier 1 dispatch.
pub struct NameInput<'a> {
    pub raw_bytes: &'a [u8],
    /// True when GP bit 11 (EFS) is set on the entry's general
    /// purpose flags. Either guarantees UTF-8 encoding of `raw_bytes`.
    pub utf8_flag: bool,
    /// Decoded payload of the Info-ZIP Unicode Path extra field
    /// (`0x7075`) when present + CRC32-validated. Takes precedence
    /// over `raw_bytes` because authoring tools that emit 0x7075
    /// usually keep `raw_bytes` in legacy MBCS for older readers.
    pub unicode_path_extra: Option<&'a str>,
}

/// Decode every name with the policy decided by [`detect_archive_encoding`].
/// Most callers only need this entry point.
pub fn decode_names(
    inputs: &[NameInput<'_>],
    user_override: LegacyCodepageOverride,
) -> (Vec<String>, NameEncoding) {
    let encoding = detect_archive_encoding(inputs, user_override);
    let decoded = inputs
        .iter()
        .map(|n| decode_one(n, encoding))
        .collect();
    (decoded, encoding)
}

/// Decode a single name once the archive-level encoding decision has
/// been made. Tier 1 dispatch is still per-entry so a mixed archive
/// (some EFS bit 11, some not) decodes correctly.
pub fn decode_one(input: &NameInput<'_>, fallback: NameEncoding) -> String {
    if let Some(unicode) = input.unicode_path_extra {
        return unicode.to_string();
    }
    if input.utf8_flag {
        if let Ok(s) = std::str::from_utf8(input.raw_bytes) {
            return s.to_string();
        }
        // bit 11 lied — keep going through the cascade.
    }
    if let Ok(s) = std::str::from_utf8(input.raw_bytes) {
        return s.to_string();
    }
    match fallback {
        NameEncoding::Utf8 => String::from_utf8_lossy(input.raw_bytes).into_owned(),
        NameEncoding::Legacy(cp) => {
            let (cow, _, _) = cp.encoding().decode(input.raw_bytes);
            cow.into_owned()
        }
    }
}

/// Pick a codepage for the legacy-MBCS path. Honours the user
/// override; in `Auto` mode it feeds every non-ASCII entry into
/// `chardetng`, then falls back to the OS locale, then to UTF-8 as a
/// last resort.
pub fn detect_archive_encoding(
    inputs: &[NameInput<'_>],
    user_override: LegacyCodepageOverride,
) -> NameEncoding {
    match user_override {
        LegacyCodepageOverride::Utf8 => return NameEncoding::Utf8,
        LegacyCodepageOverride::Legacy(cp) => return NameEncoding::Legacy(cp),
        LegacyCodepageOverride::Auto => {}
    }

    // If every entry is either UTF-8 marked or valid UTF-8 by
    // happenstance, we never need a legacy codepage.
    let all_utf8 = inputs.iter().all(|n| {
        n.unicode_path_extra.is_some()
            || n.utf8_flag
            || std::str::from_utf8(n.raw_bytes).is_ok()
    });
    if all_utf8 {
        return NameEncoding::Utf8;
    }

    // Run chardetng on every non-ASCII name byte sequence. The
    // detector handles short-input cases poorly when fed one line at
    // a time, so we batch the whole central directory.
    let mut detector = chardetng::EncodingDetector::new();
    let mut fed_any = false;
    for input in inputs {
        if input.unicode_path_extra.is_some() {
            continue;
        }
        if std::str::from_utf8(input.raw_bytes).is_ok() {
            continue;
        }
        detector.feed(input.raw_bytes, false);
        detector.feed(b"\n", false); // separator between names
        fed_any = true;
    }
    if fed_any {
        detector.feed(&[], true);
    }
    let detected = detector.guess(None, true);

    // Map detector output to our enum. chardetng can return labels
    // outside the four codepages we care about (windows-125x for
    // example) — those fall through to locale_default.
    if detected == EUC_KR {
        return NameEncoding::Legacy(LegacyCodepage::Cp949);
    }
    if detected == SHIFT_JIS {
        return NameEncoding::Legacy(LegacyCodepage::ShiftJis);
    }
    if detected == GBK || detected.name() == "gb18030" {
        return NameEncoding::Legacy(LegacyCodepage::Gbk);
    }
    if detected == BIG5 {
        return NameEncoding::Legacy(LegacyCodepage::Big5);
    }
    // Non-CJK detection (windows-1252 etc.) — likely a misfire on
    // very short / very ambiguous inputs. Fall through to locale.
    let by_locale = locale_codepage();
    if let Some(cp) = by_locale {
        return NameEncoding::Legacy(cp);
    }
    if detected == UTF_8 {
        NameEncoding::Utf8
    } else {
        // Last resort: keep UTF-8-lossy decoding (replacement chars).
        NameEncoding::Utf8
    }
}

/// Read the OS locale and map BCP-47 prefix → legacy codepage.
/// Returns `None` for locales without a clear CJK mapping (Anglo,
/// European, RTL etc.) so the caller can stay on UTF-8.
fn locale_codepage() -> Option<LegacyCodepage> {
    let raw = sys_locale::get_locale()?;
    let lc = raw.to_ascii_lowercase();
    if lc.starts_with("ko") {
        Some(LegacyCodepage::Cp949)
    } else if lc.starts_with("ja") {
        Some(LegacyCodepage::ShiftJis)
    } else if lc.starts_with("zh-cn") || lc.starts_with("zh_cn") || lc == "zh" {
        Some(LegacyCodepage::Gbk)
    } else if lc.starts_with("zh-tw") || lc.starts_with("zh_tw") || lc.starts_with("zh-hk") {
        Some(LegacyCodepage::Big5)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name<'a>(bytes: &'a [u8]) -> NameInput<'a> {
        NameInput {
            raw_bytes: bytes,
            utf8_flag: false,
            unicode_path_extra: None,
        }
    }

    #[test]
    fn ascii_only_archive_decodes_as_utf8() {
        let inputs = vec![name(b"alpha.txt"), name(b"beta.bin")];
        let (names, enc) = decode_names(&inputs, LegacyCodepageOverride::Auto);
        assert_eq!(enc, NameEncoding::Utf8);
        assert_eq!(names, vec!["alpha.txt".to_string(), "beta.bin".to_string()]);
    }

    #[test]
    fn utf8_flag_is_trusted_when_set() {
        // "한글.txt" in UTF-8
        let kor_utf8 = "한글.txt".as_bytes();
        let inputs = vec![NameInput {
            raw_bytes: kor_utf8,
            utf8_flag: true,
            unicode_path_extra: None,
        }];
        let (names, _) = decode_names(&inputs, LegacyCodepageOverride::Auto);
        assert_eq!(names[0], "한글.txt");
    }

    #[test]
    fn cp949_archive_decodes_via_detector_or_locale() {
        // "한글.txt" in CP949 (EUC-KR superset).
        // 한 = 0xC7 0xD1, 글 = 0xB1 0xDB
        let bytes = b"\xC7\xD1\xB1\xDB.txt";
        let inputs = vec![name(bytes), name(bytes)];
        let (_, enc) = decode_names(&inputs, LegacyCodepageOverride::Auto);
        // We don't assert specific codepage because chardetng's
        // result depends on input volume + OS locale of the test
        // host. We DO assert that we picked legacy (not UTF-8).
        assert!(matches!(enc, NameEncoding::Legacy(_)));
    }

    #[test]
    fn override_pins_codepage() {
        let bytes = b"\xC7\xD1\xB1\xDB.txt"; // CP949 한글.txt
        let inputs = vec![name(bytes)];
        let (names, enc) = decode_names(
            &inputs,
            LegacyCodepageOverride::Legacy(LegacyCodepage::Cp949),
        );
        assert_eq!(enc, NameEncoding::Legacy(LegacyCodepage::Cp949));
        assert_eq!(names[0], "한글.txt");
    }
}
