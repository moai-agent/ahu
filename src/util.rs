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

/// Largest configuration file ahu will read into a digest.
///
/// Agent configuration is prose, JSON, and small scripts. This is far above
/// anything legitimate and far below a size that matters.
pub const MAX_CONFIG_BYTES: u64 = 64 * 1024 * 1024;

/// Lowercase hex SHA-256 of a file's contents.
///
/// Streamed rather than read whole, and capped. A repository can commit a file
/// of any size at a configuration path, and this runs on every one of them —
/// once in `snapshot::collect` and twice more per file in `materialize` — so
/// `ahu`, `ahu inventory`, `ahu doctor` and every launch would each read it end
/// to end. A committed multi-gigabyte `.claude/x` was enough to stall all of
/// them before any preview was shown.
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
    let first = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let cleaned: String = first
        .chars()
        .map(|c| if is_display_hostile(c) { ' ' } else { c })
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

/// Make a path safe to print to a terminal.
///
/// `display_safe` is applied to every repository-derived *string* ahu prints,
/// but `Path::display()` was used raw. Under today's threat model the checkout
/// path is chosen by whoever ran `git clone`, not by the repository, so this is
/// not exploitable from a clone alone — it is one refactor away from being so,
/// and a renderer should not have two rules for the same job.
pub fn display_path(path: &Path) -> String {
    display_safe(&path.to_string_lossy())
}
