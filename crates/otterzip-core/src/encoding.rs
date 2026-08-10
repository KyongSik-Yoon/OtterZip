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

    // Neither the detector nor the locale produced a CJK answer, yet we
    // already know at least one name is NOT valid UTF-8 — the `all_utf8`
    // early return above is the only way out otherwise. Returning
    // `NameEncoding::Utf8` here would hand those bytes to
    // `String::from_utf8_lossy` and emit a filename of U+FFFD replacement
    // characters. That is never the right answer; it is only ever "we gave
    // up".
    //
    // This used to be the common outcome OFF Windows. `sys_locale` reads
    // `LANG`/`LC_ALL`, which on a server, a container, a CI runner, or any
    // desktop not set to a CJK locale says `C` or `en_US.UTF-8` — so a
    // Korean user extracting a Bandizip-era ZIP on Linux got mojibake even
    // though the very same archive decoded correctly on Windows, where the
    // locale is reliably `ko-KR`. The locale is a hint about the USER, not
    // about the ARCHIVE; when it is silent we can still ask the bytes.
    score_codepage_by_content(inputs).map_or(NameEncoding::Utf8, NameEncoding::Legacy)
}

/// Last-resort codepage pick: decode the non-UTF-8 names under each
/// candidate and keep the one that yields the most plausible text.
///
/// A codepage is disqualified outright if it cannot decode the bytes
/// without replacement characters. Among survivors — and the four CJK
/// codepages accept most byte pairs, so there are usually several — the
/// winner is the one that produces the most characters in its own
/// *signature* script, because that is the part they disagree about:
///
///   * CP949 alone produces Hangul.
///   * Shift_JIS alone produces kana.
///   * GBK and Big5 differ from each other only in which Han characters
///     they produce, which no heuristic can separate; the tie-break below
///     is what decides, and it is deliberately the same order the detector
///     itself prefers.
///
/// Half-width katakana (U+FF61..U+FF9F) is explicitly NOT counted as a kana
/// signal even though it is what Shift_JIS maps the single bytes 0xA1..0xDF
/// to. Those are exactly the bytes CP949 uses as lead bytes, so every
/// Korean name "decodes cleanly" as a run of half-width katakana — the
/// single most common misfire in this whole cascade. Real Japanese
/// filenames use full-width kana.
fn score_codepage_by_content(inputs: &[NameInput<'_>]) -> Option<LegacyCodepage> {
    const CANDIDATES: [LegacyCodepage; 4] = [
        LegacyCodepage::Cp949,
        LegacyCodepage::ShiftJis,
        LegacyCodepage::Gbk,
        LegacyCodepage::Big5,
    ];

    let mut best: Option<(LegacyCodepage, u32)> = None;
    for cp in CANDIDATES {
        let mut score = 0u32;
        let mut usable = true;
        for input in inputs {
            if input.unicode_path_extra.is_some() || std::str::from_utf8(input.raw_bytes).is_ok() {
                continue;
            }
            let (text, _, had_errors) = cp.encoding().decode(input.raw_bytes);
            if had_errors {
                usable = false;
                break;
            }
            score += signature_score(cp, &text);
        }
        if !usable || score == 0 {
            continue;
        }
        // Strictly greater keeps the CANDIDATES order as the tie-break.
        if best.is_none_or(|(_, b)| score > b) {
            best = Some((cp, score));
        }
    }
    best.map(|(cp, _)| cp)
}

/// Count the characters in `text` that only this codepage would have
/// produced. Han ideographs score for every CJK codepage (they are shared),
/// so they raise all candidates equally and never decide the winner on
/// their own — they only serve to beat a zero.
fn signature_score(cp: LegacyCodepage, text: &str) -> u32 {
    text.chars()
        .map(|c| match cp {
            LegacyCodepage::Cp949 => match c {
                // Hangul syllables, compatibility jamo, jamo.
                '\u{AC00}'..='\u{D7A3}' | '\u{3130}'..='\u{318F}' | '\u{1100}'..='\u{11FF}' => 4,
                _ => u32::from(is_han(c)),
            },
            LegacyCodepage::ShiftJis => match c {
                // Full-width hiragana + katakana. Half-width katakana is
                // deliberately absent — see the doc comment above.
                '\u{3040}'..='\u{30FF}' => 4,
                _ => u32::from(is_han(c)),
            },
            LegacyCodepage::Gbk | LegacyCodepage::Big5 => u32::from(is_han(c)),
        })
        .sum()
}

fn is_han(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}')
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

    /// The regression that made this whole content-scoring fallback
    /// necessary: on any host whose locale is not CJK — every CI runner,
    /// container and non-Korean Linux desktop — `chardetng` misfires on
    /// four bytes and `locale_codepage()` returns `None`, so the cascade
    /// used to fall through to `NameEncoding::Utf8` and decode a perfectly
    /// good Korean filename into replacement characters.
    ///
    /// The assertion is on the DECODED NAME, not merely on "some legacy
    /// codepage was chosen": picking Shift_JIS here would also satisfy
    /// `matches!(enc, Legacy(_))` while still producing garbage
    /// (`ﾇﾑｱﾛ.txt`), which is the specific wrong answer this scorer exists
    /// to rule out.
    #[test]
    fn cp949_names_decode_without_a_korean_locale() {
        let bytes = b"\xC7\xD1\xB1\xDB.txt"; // 한글.txt
        let inputs = vec![name(bytes)];
        assert_eq!(
            score_codepage_by_content(&inputs),
            Some(LegacyCodepage::Cp949)
        );
        let (names, enc) = decode_names(&inputs, LegacyCodepageOverride::Auto);
        assert_eq!(enc, NameEncoding::Legacy(LegacyCodepage::Cp949));
        assert_eq!(names[0], "한글.txt");
    }

    #[test]
    fn shift_jis_names_decode_without_a_japanese_locale() {
        // "日本語.txt" in Shift_JIS.
        let bytes = b"\x93\xFA\x96\x7B\x8C\xEA.txt";
        let inputs = vec![name(bytes)];
        let (names, _) = decode_names(&inputs, LegacyCodepageOverride::Auto);
        assert_eq!(names[0], "日本語.txt");
    }

    #[test]
    fn ascii_only_input_yields_no_content_guess() {
        // Nothing to score — every name is valid UTF-8, so the scorer must
        // decline rather than invent a codepage.
        let inputs = vec![name(b"plain.txt")];
        assert_eq!(score_codepage_by_content(&inputs), None);
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
