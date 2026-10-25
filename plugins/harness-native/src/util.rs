pub(crate) use crate::{Error, Result};
use sha2::{Digest, Sha256};
pub(crate) fn digest_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn is_semver(value: &str) -> bool {
    // `1.2.3-rc.1+build.5`. Validating only the dotted core would let arbitrary
    // bytes — including ESC — ride along in the pre-release or build metadata,
    // and the version is printed as part of `name@version`.
    let (without_build, build) = match value.split_once('+') {
        Some((head, tail)) => (head, Some(tail)),
        None => (value, None),
    };
    let (core, pre_release) = match without_build.split_once('-') {
        Some((head, tail)) => (head, Some(tail)),
        None => (without_build, None),
    };

    let mut count = 0;
    for part in core.split('.') {
        count += 1;
        if count > 3 {
            return false;
        }
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        if part.len() > 1 && part.starts_with('0') {
            return false;
        }
    }
    if count != 3 {
        return false;
    }

    // Pre-release and build are dot-separated identifiers of ASCII
    // alphanumerics and hyphens. Nothing else, and never empty.
    for section in [pre_release, build].into_iter().flatten() {
        if section.is_empty() {
            return false;
        }
        for identifier in section.split('.') {
            if identifier.is_empty()
                || !identifier
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                return false;
            }
        }
    }
    // Numeric prerelease identifiers cannot have leading zeros. Build metadata
    // and prerelease identifiers containing non-digits may still start with zero.
    if pre_release.is_some_and(|section| {
        section.split('.').any(|identifier| {
            identifier.len() > 1
                && identifier.starts_with('0')
                && identifier.bytes().all(|byte| byte.is_ascii_digit())
        })
    }) {
        return false;
    }
    true
}

#[cfg(test)]
pub(crate) fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace("'", "'\"'\"'"))
}

pub fn display_safe(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if is_display_hostile(ch) {
            push_escape(&mut out, ch);
        } else {
            out.push(ch);
        }
    }
    out
}

/// Whether a character must never reach the terminal as itself.
///
/// Two groups, for two different reasons. Control characters (including C1)
/// move the cursor, clear the screen, or change colour. The rest are invisible
/// or direction-changing: on a bidi-aware terminal U+202E makes a hook's
/// displayed program name render as something other than what it is, and the
/// zero-width family lets two different strings look identical.
fn is_display_hostile(ch: char) -> bool {
    let code = ch as u32;
    ch.is_control()
        || (0x80..=0x9f).contains(&code)
        || matches!(
            code,
            0x00ad                  // soft hyphen
            | 0x061c                // arabic letter mark
            | 0x200b..=0x200f       // zero width space .. right-to-left mark
            | 0x2028 | 0x2029       // line and paragraph separator
            | 0x202a..=0x202e       // bidi embedding and override
            | 0x2060..=0x2064       // word joiner and invisible operators
            | 0x2066..=0x2069       // bidi isolates
            | 0xfeff // zero width no-break space
        )
}

/// Render one hostile character visibly.
///
/// `\xNN` cannot name anything above 0xFF without being ambiguous, so wider
/// characters use the `\u{...}` form Rust itself uses.
fn push_escape(out: &mut String, ch: char) {
    let code = ch as u32;
    if code <= 0xff {
        out.push_str(&format!("\\x{code:02x}"));
    } else {
        out.push_str(&format!("\\u{{{code:04x}}}"));
    }
}
