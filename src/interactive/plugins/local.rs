//! Bounded metadata reads for explicitly supplied local Rust packages.
use super::PluginStatus;
use crate::util::{Error, ErrorKind, Result};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

const MAX_PACKAGES: usize = 32;
const MAX_METADATA_BYTES: u64 = 64 * 1024;
const MAX_MANIFEST_NODES: usize = 8192;
const MAX_MANIFEST_STRING_BYTES: usize = 1024 * 1024;
pub(super) const LOADING_BLOCKER: &str = "This local package has no executable host declaration; source-only packages cannot be activated. No backend was substituted.";

#[derive(Deserialize)]
struct Manifest {
    okf_version: serde_json::Value,
    #[serde(rename = "type")]
    kind: String,
    schema_version: u32,
    id: String,
    package: String,
    host_api: u32,
    cargo_manifest: String,
    executable: Option<crate::interactive::external::Executable>,
    entrypoint: Option<crate::plugin_exec::Entrypoint>,
    // General OKF metadata is preserved for inspection but never interpreted
    // as host capabilities or execution settings.
    #[serde(default, flatten)]
    metadata: BTreeMap<String, serde_json::Value>,
}

enum UniqueYamlValue {
    Null(()),
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Sequence(Vec<BudgetedYamlValue>),
    Mapping(UniqueYamlMap),
}

struct BudgetedYamlValue(UniqueYamlValue);

thread_local! {
    static MANIFEST_NODE_COUNT: Cell<usize> = const { Cell::new(0) };
    static MANIFEST_STRING_BYTES: Cell<usize> = const { Cell::new(0) };
}

fn account_manifest_string<E: de::Error>(value: &str) -> std::result::Result<(), E> {
    let within_budget = MANIFEST_STRING_BYTES.with(|count| {
        let next = count.get().saturating_add(value.len());
        if next > MAX_MANIFEST_STRING_BYTES {
            false
        } else {
            count.set(next);
            true
        }
    });
    if within_budget {
        Ok(())
    } else {
        Err(E::custom(
            "YAML frontmatter expands beyond the metadata string-byte limit",
        ))
    }
}

impl<'de> Deserialize<'de> for BudgetedYamlValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let within_budget = MANIFEST_NODE_COUNT.with(|count| {
            let next = count.get().saturating_add(1);
            if next > MAX_MANIFEST_NODES {
                false
            } else {
                count.set(next);
                true
            }
        });
        if !within_budget {
            return Err(de::Error::custom(
                "YAML frontmatter expands beyond the metadata node limit",
            ));
        }
        deserializer
            .deserialize_any(UniqueYamlValueVisitor)
            .map(Self)
    }
}

struct UniqueYamlValueVisitor;

impl<'de> Visitor<'de> for UniqueYamlValueVisitor {
    type Value = UniqueYamlValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded YAML value with unique string mapping keys")
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(UniqueYamlValue::Null(()))
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(UniqueYamlValue::Null(()))
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(UniqueYamlValue::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(UniqueYamlValue::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(UniqueYamlValue::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(UniqueYamlValue::Number)
            .ok_or_else(|| E::custom("YAML numbers must be finite"))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        account_manifest_string::<E>(value)?;
        Ok(UniqueYamlValue::String(value.to_owned()))
    }

    fn visit_borrowed_str<E>(self, value: &'de str) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value)
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        account_manifest_string::<E>(&value)?;
        Ok(UniqueYamlValue::String(value))
    }

    fn visit_seq<A>(self, mut access: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: de::SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = access.next_element::<BudgetedYamlValue>()? {
            values.push(value);
        }
        Ok(UniqueYamlValue::Sequence(values))
    }

    fn visit_map<A>(self, access: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        UniqueMapVisitor
            .visit_map(access)
            .map(UniqueYamlValue::Mapping)
    }
}

struct UniqueYamlMap(BTreeMap<String, BudgetedYamlValue>);

impl<'de> Deserialize<'de> for UniqueYamlMap {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(UniqueMapVisitor)
    }
}

struct UniqueMapVisitor;

impl<'de> Visitor<'de> for UniqueMapVisitor {
    type Value = UniqueYamlMap;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a YAML mapping with unique string keys")
    }

    fn visit_map<A>(self, mut access: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut entries = BTreeMap::new();
        while let Some(key) = access.next_key::<BudgetedYamlValue>()? {
            let key = match key.0 {
                UniqueYamlValue::String(value) => value,
                _ => return Err(de::Error::custom("YAML mapping keys must be strings")),
            };
            if entries.contains_key(&key) {
                return Err(de::Error::custom("duplicate YAML mapping key"));
            }
            entries.insert(key, access.next_value()?);
        }
        Ok(UniqueYamlMap(entries))
    }
}

impl UniqueYamlValue {
    fn into_json(self) -> serde_json::Value {
        match self {
            Self::Null(()) => serde_json::Value::Null,
            Self::Bool(value) => serde_json::Value::Bool(value),
            Self::Number(value) => serde_json::Value::Number(value),
            Self::String(value) => serde_json::Value::String(value),
            Self::Sequence(values) => serde_json::Value::Array(
                values
                    .into_iter()
                    .map(|value| value.0.into_json())
                    .collect(),
            ),
            Self::Mapping(UniqueYamlMap(values)) => serde_json::Value::Object(
                values
                    .into_iter()
                    .map(|(key, value)| (key, value.0.into_json()))
                    .collect(),
            ),
        }
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(message).with_kind(ErrorKind::Usage)
}

fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

// Pin every directory, starting at /; never follow a symlink in a supplied
// root, a manifest, or a package path. Nonblocking opens also reject FIFOs
// without waiting for a writer. These handles are for inspection, not loading.
fn open_child(parent: &File, name: &OsStr, directory: bool) -> Result<File> {
    let name =
        std::ffi::CString::new(name.as_bytes()).map_err(|_| invalid("plugin path contains NUL"))?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    // SAFETY: parent is live, name is NUL-terminated, and no creation is requested.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(invalid(
            "plugin paths must exist and contain only accessible directories and regular files, without symlinks",
        ));
    }
    // SAFETY: openat returned a fresh descriptor owned by this File.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata()?;
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(invalid("plugin metadata must be a regular file"));
    }
    Ok(file)
}

fn read_text(mut file: File) -> Result<String> {
    let bytes = crate::util::read_limited(&mut file, MAX_METADATA_BYTES)
        .map_err(|_| invalid("plugin metadata is unreadable or exceeds 64 KiB"))?;
    String::from_utf8(bytes).map_err(|_| invalid("plugin metadata must be UTF-8"))
}

fn read_toml(file: File) -> Result<toml::Value> {
    // Cargo metadata remains TOML. Never echo untrusted file contents.
    toml::from_str(&read_text(file)?).map_err(|_| invalid("Cargo.toml must be valid TOML"))
}

fn read_manifest(file: File) -> Result<Manifest> {
    let text = read_text(file)?;
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .ok_or_else(|| {
            invalid("ahu-plugin.md must begin with YAML frontmatter delimited by --- lines")
        })?;
    let mut end = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            MANIFEST_NODE_COUNT.with(|count| count.set(0));
            MANIFEST_STRING_BYTES.with(|count| count.set(0));
            let parsed_result = yaml_serde::from_str::<BudgetedYamlValue>(&rest[..end]);
            MANIFEST_NODE_COUNT.with(|count| count.set(0));
            MANIFEST_STRING_BYTES.with(|count| count.set(0));
            let parsed = parsed_result.map_err(|_| {
                invalid("ahu-plugin.md has invalid YAML frontmatter (required fields must be present; duplicate keys are refused)")
            })?;
            let manifest: Manifest = serde_json::from_value(parsed.0.into_json()).map_err(|_| {
                invalid("ahu-plugin.md has invalid YAML frontmatter (required fields must be present; duplicate keys are refused)")
            })?;
            let valid_okf_version = match &manifest.okf_version {
                serde_json::Value::String(version) => version == "0.2",
                serde_json::Value::Number(version) => version.as_f64() == Some(0.2),
                _ => false,
            };
            if !valid_okf_version || manifest.kind != "ahu:plugin" {
                return Err(invalid(
                    "ahu-plugin.md requires okf_version: \"0.2\" and type: ahu:plugin",
                ));
            }
            // The Markdown body is documentation only, never configuration or code.
            return Ok(manifest);
        }
        end += line.len();
    }
    Err(invalid(
        "ahu-plugin.md YAML frontmatter is missing its closing --- delimiter",
    ))
}

fn open_root(root: &Path) -> Result<File> {
    if !root.is_absolute() || root.components().count() > 64 {
        return Err(invalid(
            "AHU_PLUGIN_PATH requires absolute package directories (at most 64 path components)",
        ));
    }
    let text = root
        .to_str()
        .ok_or_else(|| invalid("plugin root must be UTF-8"))?;
    if text.split('/').any(|part| part == "." || part == "..") {
        return Err(invalid("plugin root cannot contain . or .. components"));
    }
    let mut directory = File::open("/")?;
    for component in root.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => directory = open_child(&directory, name, true)?,
            _ => return Err(invalid("plugin root cannot contain . or .. components")),
        }
    }
    Ok(directory)
}

pub(crate) fn read_package(root: &Path) -> Result<PluginStatus> {
    let directory = open_root(root)?;
    let manifest = read_manifest(open_child(&directory, OsStr::new("ahu-plugin.md"), false)?)?;
    if manifest.schema_version != 1 || manifest.host_api != 1 {
        return Err(invalid(
            "local plugins require manifest schema_version = 1 and host_api = 1",
        ));
    }
    if !identity(&manifest.id) || !identity(&manifest.package) || manifest.id == "none" {
        return Err(invalid(
            "plugin id and package must be 1-64 lowercase ASCII letters, digits, hyphens or underscores, starting with a letter; id 'none' is reserved",
        ));
    }
    let parts: Vec<_> = manifest.cargo_manifest.split('/').collect();
    if parts.len() > 32
        || parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains('\\'))
        || parts.last() != Some(&"Cargo.toml")
    {
        return Err(invalid(
            "cargo_manifest must be a relative path ending in Cargo.toml, with at most 32 normal components and no . or ..",
        ));
    }
    let mut parent = directory.try_clone()?;
    for part in &parts[..parts.len() - 1] {
        parent = open_child(&parent, OsStr::new(part), true)?;
    }
    let cargo = read_toml(open_child(&parent, OsStr::new("Cargo.toml"), false)?)?;
    if cargo
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        != Some(manifest.package.as_str())
    {
        return Err(invalid(
            "Cargo.toml [package].name must match the declared plugin package",
        ));
    }
    if let Some(executable) = &manifest.executable {
        executable.validate()?;
        validate_executable(&open_relative(&directory, &executable.path)?)?;
    }
    if let Some(entrypoint) = &manifest.entrypoint {
        entrypoint.validate()?;
        validate_executable(&open_relative(&directory, &entrypoint.path)?)?;
    }
    let activation_blocker = if manifest.executable.is_some() {
        None
    } else if manifest.entrypoint.is_some() {
        Some(
            "This package has a generic entrypoint but no session host; use ahu plugins invoke ID --input-file PATH|- or the plugin_exec host API.",
        )
    } else {
        Some(LOADING_BLOCKER)
    };
    Ok(PluginStatus {
        id: manifest.id,
        package: manifest.package,
        host_api: manifest.host_api,
        selected: false,
        selection_source: "not-selected",
        origin: "local-manifest",
        manifest: Some(root.join("ahu-plugin.md")),
        cargo_manifest: Some(root.join(manifest.cargo_manifest)),
        executable: manifest.executable,
        entrypoint: manifest.entrypoint,
        metadata: manifest.metadata,
        activation_blocker,
    })
}

pub(crate) fn discover() -> Result<Vec<PluginStatus>> {
    let Some(value) = std::env::var_os("AHU_PLUGIN_PATH") else {
        return Ok(Vec::new());
    };
    if value.len() > 64 * 1024 {
        return Err(invalid("AHU_PLUGIN_PATH exceeds 64 KiB"));
    }
    let mut packages = Vec::new();
    let mut identities = BTreeSet::from(["cmux".to_string()]);
    for root in std::env::split_paths(&value) {
        if packages.len() == MAX_PACKAGES {
            return Err(invalid("AHU_PLUGIN_PATH exceeds 32 package directories"));
        }
        let package = read_package(&root)?;
        if !identities.insert(package.id.clone()) {
            return Err(invalid(format!(
                "duplicate plugin identity '{}' (cmux is reserved for the compiled package)",
                package.id
            )));
        }
        packages.push(package);
    }
    packages.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(packages)
}

// The package root and every relative component are pinned again at activation.
// Copying from this handle avoids executing a subsequently replaced pathname.
pub(crate) fn executable_file(status: &PluginStatus) -> Result<File> {
    let root = status
        .manifest
        .as_ref()
        .and_then(|p| p.parent())
        .ok_or_else(|| invalid("local package has no manifest root"))?;
    let executable = status
        .executable
        .as_ref()
        .ok_or_else(|| invalid(LOADING_BLOCKER))?;
    executable.validate()?;
    let file = open_relative(&open_root(root)?, &executable.path)?;
    validate_executable(&file)?;
    Ok(file)
}

fn open_relative(directory: &File, path: &str) -> Result<File> {
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() > 32
        || parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains('\\'))
    {
        return Err(invalid(
            "executable path requires at most 32 normal relative components",
        ));
    }
    let mut parent = directory.try_clone()?;
    for part in &parts[..parts.len() - 1] {
        parent = open_child(&parent, OsStr::new(part), true)?;
    }
    open_child(&parent, OsStr::new(parts[parts.len() - 1]), false)
}

fn validate_executable(file: &File) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = file.metadata()?;
    let mode = metadata.permissions().mode();
    if mode & 0o111 == 0 || mode & 0o6000 != 0 || metadata.len() > 64 * 1024 * 1024 {
        return Err(invalid(
            "plugin executable must be executable, non-setid, and at most 64 MiB",
        ));
    }
    Ok(())
}

/// Reopen a generic entrypoint through the same no-symlink package boundary.
pub(crate) fn entrypoint_file(status: &PluginStatus) -> Result<File> {
    let root = status
        .manifest
        .as_ref()
        .and_then(|p| p.parent())
        .ok_or_else(|| invalid("package has no manifest root"))?;
    let spec = status
        .entrypoint
        .as_ref()
        .ok_or_else(|| invalid("package has no generic entrypoint"))?;
    spec.validate()?;
    let file = open_relative(&open_root(root)?, &spec.path)?;
    validate_executable(&file)?;
    Ok(file)
}
