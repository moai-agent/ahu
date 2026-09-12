//! Small shared helpers: the crate error type, shell quoting, and digests.

use std::fmt;
use std::path::{Path, PathBuf};

/// Stable exit categories for command failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrorKind {
    Usage = 2,
    UnknownAgent = 3,
    Prerequisite = 4,
    RunFailure = 5,
}

#[derive(Debug)]
/// A failure category and an actionable message for the person running ahu.
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    /// Runtime validation, persistence, and subprocess failures default to run failure.
    /// User input and unavailable prerequisites are classified at their boundaries.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ErrorKind::RunFailure,
        }
    }
}

impl Error {
    pub fn with_kind(mut self, kind: ErrorKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Error::new(value.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Error::new(format!("invalid JSON: {value}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Build an [`Error`] with `format!` syntax.
#[macro_export]
macro_rules! bail {
    (kind: $kind:expr, $($arg:tt)*) => {
        return Err($crate::util::Error::new(format!($($arg)*)).with_kind($kind))
    };
    ($($arg:tt)*) => {
        return Err($crate::util::Error::new(format!($($arg)*)))
    };
}

/// Quote `value` so a POSIX shell passes it through as a single literal word.
///
/// ahu only ever puts its own executable path and an ahu-owned task directory
/// into a shell-interpreted cmux startup command. Task prompts are never quoted
/// into a command line; they travel as files. This helper exists so the two
/// paths that *do* get interpolated cannot break out of their word even when
/// they contain spaces, quotes, or shell metacharacters.
pub fn shell_single_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            // Close the quote, emit an escaped quote, reopen.
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn digest_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Largest configuration file ahu will read into a digest.
///
/// Bounds hashing work for repository-controlled configuration files.
pub const MAX_CONFIG_BYTES: u64 = 64 * 1024 * 1024;

/// Lowercase hex SHA-256 of a file's contents.
///
/// Streamed and capped because snapshot collection and materialization hash
/// repository-controlled files repeatedly; file size must not imply unbounded
/// memory use or read time.
pub fn digest_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    digest_reader(&mut file, path)
}

/// Digest whatever an already-open handle yields, with the same size cap.
///
/// Separate from [`digest_file`] so a caller that has to prove the bytes it
/// hashed are the bytes it copied can do both from one descriptor, rather than
/// opening the path twice and hoping it still names the same file.
pub fn digest_reader(reader: &mut impl std::io::Read, shown: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|e| Error::new(format!("cannot read {}: {e}", shown.display())))?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > MAX_CONFIG_BYTES {
            return Err(Error::new(format!(
                "{} is larger than the {} MiB ahu will read for a configuration file.\n\
                 ahu digests every agent-configuration file it inventories, so it will not read \
                 an unbounded one. Move this file out of an agent-configuration path.",
                shown.display(),
                MAX_CONFIG_BYTES / (1024 * 1024)
            )));
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Names used for agents, branches, and cmux titles must be safe in all three.
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && !name.starts_with('-')
        && !name.starts_with('_')
}

/// A semantic version, restricted to the `MAJOR.MINOR.PATCH` core plus optional
/// pre-release and build metadata. ahu requires one on every named agent.
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
    true
}

/// Make a string safe to print to a terminal.
///
/// Repository content reaches ahu's output in many places: hook commands, agent
/// descriptions and versions, native frontmatter keys, and file names. Any of
/// those may carry ANSI escape sequences, and ahu's output is a security
/// surface — the launch preview is the only thing standing between a
/// repository's hooks and a session that runs them. A control sequence that
/// scrolls up and repaints those lines would let a repository hide its own
/// disclosure.
///
/// Every control character, including the C1 range, is rendered as a visible
/// escape, and so is every character that reorders or hides text without being
/// a control character: the bidirectional overrides and isolates that Trojan
/// Source uses, the zero-width and joiner family, and the Unicode line and
/// paragraph separators. `char::is_control` is Unicode category `Cc` only, so
/// none of those are covered by it.
///
/// This is applied at the render boundary only, so digests and stored records
/// keep the true bytes.
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
pub(crate) fn is_display_hostile_char(ch: char) -> bool {
    is_display_hostile(ch)
}

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

/// Like [`display_safe`], but keeps newlines.
///
/// For multi-line output such as an error message, where the line structure is
/// meaningful but every other control character is not. U+2028 and U+2029 are
/// still escaped: they are not the newline this keeps, and a terminal that
/// treats them as one would break a line the caller did not break.
pub fn display_safe_block(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch == '\n' {
            out.push(ch);
        } else if is_display_hostile(ch) {
            push_escape(&mut out, ch);
        } else {
            out.push(ch);
        }
    }
    out
}

/// Resolve `relative` under `root`, refusing to traverse a symlink.
///
/// Every path ahu writes to, deletes, or creates inside a repository goes
/// through this. A repository can commit a symlink at any path it likes — at
/// `.agents/ahu/agents`, at `.claude`, at `.worktrees` — and following one lets
/// the repository choose where ahu's filesystem operations land. Checking only
/// the final component is not enough: the escape is usually a parent.
///
/// `create_missing_dirs` creates intermediate directories as real directories
/// when they are absent; otherwise a missing component is simply reported back
/// through the returned path, which the caller may or may not require to exist.
pub fn resolve_within(root: &Path, relative: &str, create_missing_dirs: bool) -> Result<PathBuf> {
    let parts: Vec<&str> = relative.split('/').filter(|p| !p.is_empty()).collect();
    let Some((last, directories)) = parts.split_last() else {
        return Err(Error::new(format!("empty path under {}", root.display())));
    };
    if parts.iter().any(|p| *p == ".." || *p == ".") {
        return Err(Error::new(format!(
            "{relative} is not a plain path under {}",
            root.display()
        )));
    }
    let mut current = root.to_path_buf();
    for directory in directories {
        current.push(directory);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(symlink_refusal(&current, relative));
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(Error::new(format!(
                    "{} exists and is not a directory.",
                    current.display()
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !create_missing_dirs {
                    return Err(Error::new(format!("{} does not exist.", current.display())));
                }
                std::fs::create_dir(&current)
                    .map_err(|e| Error::new(format!("cannot create {}: {e}", current.display())))?;
            }
            Err(e) => {
                return Err(Error::new(format!(
                    "cannot inspect {}: {e}",
                    current.display()
                )));
            }
        }
    }
    current.push(last);
    if let Ok(meta) = std::fs::symlink_metadata(&current)
        && meta.file_type().is_symlink()
    {
        return Err(symlink_refusal(&current, relative));
    }
    Ok(current)
}

/// Resolve `relative` under `root` for *reading*, refusing to traverse a symlink.
///
/// [`resolve_within`] guards every path ahu writes to. Reads need the same
/// guard for the same reason — a repository can commit a symlink at
/// `.agents/ahu/config.toml` or `.agents/ahu/agents/x.toml`, and reading
/// through one lets the repository choose which file ahu opens and then quotes
/// back in a parse error. That is an arbitrary-file read with the contents
/// disclosed, from nothing more than checking out a repository.
///
/// Reads differ from writes in one way: a component that does not exist is the
/// ordinary "not configured yet" case, not a failure. So absence is reported as
/// `Ok(None)` and only a symlink, or a non-directory where a directory must be,
/// is an error.
///
/// A symlink swapped in between this call and the open is still possible. The
/// committed-symlink case — the one a repository controls — is closed here;
/// winning that race additionally requires write access to the checkout while
/// ahu is running.
pub fn resolve_existing_within(root: &Path, relative: &str) -> Result<Option<PathBuf>> {
    let parts: Vec<&str> = relative.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err(Error::new(format!("empty path under {}", root.display())));
    }
    if parts.iter().any(|p| *p == ".." || *p == ".") {
        return Err(Error::new(format!(
            "{relative} is not a plain path under {}",
            root.display()
        )));
    }
    let mut current = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        current.push(part);
        let last = index + 1 == parts.len();
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(symlink_refusal(&current, relative));
            }
            Ok(meta) if last || meta.is_dir() => {}
            Ok(_) => {
                return Err(Error::new(format!(
                    "{} exists and is not a directory.",
                    current.display()
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(Error::new(format!(
                    "cannot inspect {}: {e}",
                    current.display()
                )));
            }
        }
    }
    Ok(Some(current))
}

/// The message shown when ahu refuses to act through a symlink.
pub fn symlink_refusal(found_at: &Path, relative: &str) -> Error {
    let points_to = std::fs::read_link(found_at)
        .map(|t| t.to_string_lossy().to_string())
        .unwrap_or_else(|_| "an unreadable target".to_string());
    Error::new(format!(
        "refusing to act through a symlink.\n\
         {} is a symlink pointing at {points_to}, and ahu was about to use it for {relative}.\n\
         Following it would let this repository choose where ahu reads, writes, or deletes, so \
         nothing was done. Inspect that path in the repository before trying again.",
        found_at.display()
    ))
}

/// Collapse a prompt into a short single-line task title.
///
/// Only the first nonempty line is used and it is truncated, so a pasted prompt
/// never lands in a terminal title in full.
///
/// Every character [`display_safe`] would escape is replaced here instead, and
/// before truncation so the length bound still holds. This title is the one
/// repository- and prompt-derived string that does *not* reach the terminal
/// through a renderer: it becomes the cmux workspace name, passed to
/// `cmux new-workspace --name`, and cmux paints it in the sidebar. Filtering
/// only `char::is_control` left the bidi overrides and the zero-width family
/// intact, so a title could render as something other than what it is.
pub fn task_title_from_prompt(prompt: &str) -> String {
    prompt
        .lines()
        .map(|line| sidebar_text(line, 60))
        .find(|line| !line.is_empty())
        .unwrap_or_else(|| "untitled task".to_string())
}

/// Convert common assignment Markdown to a single, bounded plain-text label.
/// This is deliberately a display convenience, not a Markdown renderer or a
/// confidentiality filter. Callers should supply explicit metadata for long
/// assignments containing instructions that do not belong in the sidebar.
pub fn sidebar_text(text: &str, limit: usize) -> String {
    let mut plain = String::new();
    for line in text.lines() {
        let mut line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            continue;
        }
        line = line.trim_start_matches('>').trim_start();
        let heading = line.trim_start_matches('#');
        if heading.starts_with(' ') {
            line = heading.trim_start();
        }
        for marker in ["- [ ] ", "- [x] ", "- [X] ", "- ", "* ", "+ "] {
            if let Some(rest) = line.strip_prefix(marker) {
                line = rest;
                break;
            }
        }
        if let Some((number, rest)) = line.split_once(". ")
            && !number.is_empty()
            && number.bytes().all(|b| b.is_ascii_digit())
        {
            line = rest;
        }
        plain.push_str(&plain_inline(line, 0));
        plain.push(' ');
    }
    let safe: String = plain
        .chars()
        .map(|c| if is_display_hostile(c) { ' ' } else { c })
        .collect();
    let safe = safe.split_whitespace().collect::<Vec<_>>().join(" ");
    if safe.chars().count() <= limit {
        safe
    } else if limit == 0 {
        String::new()
    } else {
        format!(
            "{}…",
            safe.chars().take(limit - 1).collect::<String>().trim_end()
        )
    }
}

fn plain_inline(mut text: &str, depth: usize) -> String {
    // Bound recursion for deeply nested markup in arbitrary assignments.
    if depth >= 8 {
        return text.to_string();
    }
    let mut out = String::new();
    while !text.is_empty() {
        // Keep link labels, omitting destinations (including balanced parens).
        let link = text.strip_prefix("![").or_else(|| text.strip_prefix('['));
        if let Some(link) = link
            && let Some((label, destination)) = link.split_once("](")
        {
            let mut parentheses = 1;
            let end = destination.char_indices().find_map(|(i, c)| {
                if c == '(' {
                    parentheses += 1;
                }
                if c == ')' {
                    parentheses -= 1;
                }
                (parentheses == 0).then_some(i)
            });
            if let Some(end) = end {
                out.push_str(&plain_inline(label, depth + 1));
                text = &destination[end + 1..];
                continue;
            }
        }
        let mut matched = false;
        for marker in ["**", "__", "~~", "`", "*", "_"] {
            // Underscores within identifiers and stars within expressions are
            // ordinary text. Only remove paired markup at a word boundary.
            if !out.chars().last().is_some_and(|c| c.is_alphanumeric())
                && let Some(rest) = text.strip_prefix(marker)
                && let Some(end) = rest.find(marker)
                && end > 0
                && !rest[..end].starts_with(char::is_whitespace)
                && !rest[..end].ends_with(char::is_whitespace)
            {
                if marker == "`" {
                    out.push_str(&rest[..end]);
                } else {
                    out.push_str(&plain_inline(&rest[..end], depth + 1));
                }
                text = &rest[end + marker.len()..];
                matched = true;
                break;
            }
        }
        if !matched {
            let c = text.chars().next().expect("nonempty input");
            out.push(c);
            text = &text[c.len_utf8()..];
        }
    }
    out
}

/// Make a path safe to print to a terminal.
///
/// Apply the same control-character escaping used for repository-derived
/// strings to paths, including paths chosen by the invoking user.
pub fn display_path(path: &Path) -> String {
    display_safe(&path.to_string_lossy())
}

#[cfg(test)]
mod sidebar_tests {
    use super::*;

    #[test]
    fn link_delimiters_do_not_reset_the_inline_recursion_budget() {
        // At the boundary, the link's label must remain uninterpreted. A
        // delimiter counter shadowing depth incorrectly unwraps the emphasis.
        assert_eq!(
            plain_inline("[**leaf**](https://example.invalid/a(b))", 7),
            "**leaf**"
        );
        let mut nested = "**leaf**".to_string();
        for _ in 0..256 {
            nested = format!("[{nested}](https://example.invalid/a(b))");
        }
        assert_eq!(plain_inline(&nested, 8), nested);
        let label = sidebar_text(&nested, 60);
        assert!(label.chars().count() <= 60);
    }
}
