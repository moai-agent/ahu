//! Small shared helpers: the crate error type, shell quoting, and digests.

use std::fmt;
use std::path::{Path, PathBuf};

/// Every fallible operation in ahu returns this. The message is written for the
/// person running `ahu`, so it must say what went wrong and what to do next.
#[derive(Debug)]
pub struct Error {
    message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
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

/// Lowercase hex SHA-256 of a file's contents.
pub fn digest_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    Ok(digest_bytes(&bytes))
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
/// `\xNN` escape. This is applied at the render boundary only, so digests and
/// stored records keep the true bytes.
pub fn display_safe(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        let code = ch as u32;
        if ch.is_control() || (0x80..=0x9f).contains(&code) {
            out.push_str(&format!("\\x{code:02x}"));
        } else {
            out.push(ch);
        }
    }
    out
}

/// Like [`display_safe`], but keeps newlines.
///
/// For multi-line output such as an error message, where the line structure is
/// meaningful but every other control character is not.
pub fn display_safe_block(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        let code = ch as u32;
        if ch == '\n' {
            out.push(ch);
        } else if ch.is_control() || (0x80..=0x9f).contains(&code) {
            out.push_str(&format!("\\x{code:02x}"));
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
pub fn task_title_from_prompt(prompt: &str) -> String {
    let first = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let cleaned: String = first
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        return "untitled task".to_string();
    }
    const LIMIT: usize = 60;
    if cleaned.chars().count() <= LIMIT {
        cleaned
    } else {
        let truncated: String = cleaned.chars().take(LIMIT - 1).collect();
        format!("{}…", truncated.trim_end())
    }
}
