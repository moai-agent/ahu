//! Inert project plugin bindings. Parsing a binding never installs a package,
//! loads code, or authorizes execution. Lifecycle operations verify sources separately.
use crate::util::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

pub const DIRECTORY: &str = ".agents/ahu/plugins";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    pub source: Source,
    pub compatibility: Compatibility,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub grants: Vec<Grant>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub repository: String,
    pub revision: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
    #[serde(default, skip_serializing_if = "SourceKind::is_git")]
    pub kind: SourceKind,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    #[default]
    Git,
    Local,
}
impl SourceKind {
    fn is_git(&self) -> bool {
        *self == Self::Git
    }
}

pub fn plain_relative(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value.split('/').count() <= 32
        && value.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.eq_ignore_ascii_case(".git")
                && !part.contains(['\\', ':'])
                && !part.chars().any(char::is_control)
        })
}

pub fn local_path(value: &str) -> bool {
    value.starts_with('/') && plain_relative(&value[1..])
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Compatibility {
    pub ahu: String,
    pub host_api: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Grant {
    InteractiveBackend,
    Execute,
    ReadProjectContext,
}

pub fn parse(text: &str, expected_id: &str) -> Result<(Binding, crate::okf::Document)> {
    let document = crate::okf::Document::parse(text, "ahu:plugin-binding")?;
    let binding: Binding = document.settings()?;
    let id = &binding.id;
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || id.starts_with('-')
        || id.ends_with('-')
        || id != expected_id
    {
        return Err(Error::new(
            "plugin binding identity must match its plain filename",
        ));
    }
    if matches!(id.as_str(), "none" | "cmux") {
        return Err(Error::new(format!(
            "plugin binding id '{id}' is reserved; choose an id other than none or cmux"
        )));
    }
    if binding.schema_version != 1 || binding.compatibility.host_api != 1 {
        return Err(Error::new("unsupported plugin binding schema or host API"));
    }
    if !crate::util::is_semver(&binding.version)
        || !crate::util::is_semver(&binding.compatibility.ahu)
    {
        return Err(Error::new(
            "plugin and ahu compatibility versions must be exact semantic versions",
        ));
    }
    let source = &binding.source;
    if source
        .subdirectory
        .as_deref()
        .is_some_and(|path| !plain_relative(path))
    {
        return Err(Error::new(
            "plugin source subdirectory must be a plain relative path",
        ));
    }
    if !local_path(&source.repository) {
        let repository = url::Url::parse(&source.repository)
            .map_err(|_| Error::new("invalid plugin source repository"))?;
        if source.kind == SourceKind::Local
            || repository.scheme() != "https"
            || repository.host_str().is_none()
            || !repository.username().is_empty()
            || repository.password().is_some()
            || repository.query().is_some()
            || repository.fragment().is_some()
            || source.repository.chars().any(char::is_control)
        {
            return Err(Error::new(
                "plugin source requires an absolute local path or credential-free HTTPS Git URL",
            ));
        }
    }
    let hex = |s: &str, n| {
        s.len() == n
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    if !(hex(&source.revision, 40) || hex(&source.revision, 64)) || !hex(&source.sha256, 64) {
        return Err(Error::new(
            "plugin source requires an exact revision and SHA-256 content pin",
        ));
    }
    if source.kind == SourceKind::Local && source.revision != source.sha256 {
        return Err(Error::new(
            "local source revision must equal its SHA-256 content pin",
        ));
    }
    if binding.grants.iter().collect::<BTreeSet<_>>().len() != binding.grants.len() {
        return Err(Error::new("duplicate plugin capability grant"));
    }
    Ok((binding, document))
}

pub fn load(root: &Path, id: &str) -> Result<Option<(Binding, crate::okf::Document)>> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(Error::new("invalid plugin binding identity"));
    }
    let Some(path) = crate::util::resolve_existing_within(root, &format!("{DIRECTORY}/{id}.md"))?
    else {
        return Ok(None);
    };
    let bytes = crate::okf::read(&path)?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| Error::new("plugin binding is not UTF-8"))?;
    parse(text, id).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> Binding {
        Binding {
            schema_version: 1,
            id: "fixture".into(),
            version: "1.0.0".into(),
            source: Source {
                repository: "https://example.invalid/plugin.git".into(),
                revision: "a".repeat(40),
                sha256: "b".repeat(64),
                subdirectory: None,
                kind: SourceKind::Git,
            },
            compatibility: Compatibility {
                ahu: "0.11.0".into(),
                host_api: 1,
            },
            enabled: false,
            grants: vec![],
        }
    }
    fn render(binding: &Binding) -> String {
        crate::okf::Document::new("ahu:plugin-binding", binding, "\nDescriptive body.\n")
            .unwrap()
            .render()
            .unwrap()
    }
    #[test]
    fn version_pins_reject_leading_zero_numeric_prereleases() {
        for version in ["1.0.0-01", "1.0.0-rc.01", "1.0.0-00+build.01"] {
            for compatibility in [false, true] {
                let mut binding = binding();
                if compatibility {
                    binding.compatibility.ahu = version.into();
                } else {
                    binding.version = version.into();
                }
                let error = parse(&render(&binding), "fixture").unwrap_err();
                assert!(error.to_string().contains("exact semantic versions"));
            }
        }
        for version in ["1.0.0-0", "1.0.0-rc.0", "1.0.0-01a", "1.0.0+01"] {
            let mut binding = binding();
            binding.version = version.into();
            binding.compatibility.ahu = version.into();
            assert_eq!(parse(&render(&binding), "fixture").unwrap().0, binding);
        }
    }
    #[test]
    fn strict_identity_pins_and_grants_with_inert_metadata() {
        let binding = binding();
        let text = render(&binding);
        let (actual, mut doc) = parse(&text, "fixture").unwrap();
        assert_eq!(actual, binding);
        doc.fields.insert(
            "executable".into(),
            serde_json::json!({"command": "inert", "grants": ["all"]}),
        );
        let (actual, roundtrip) = parse(&doc.render().unwrap(), "fixture").unwrap();
        assert!(!actual.enabled);
        assert!(actual.grants.is_empty());
        assert_eq!(roundtrip, doc);
        assert!(parse(&text, "other").is_err());
        for id in ["../fixture", "UPPER", "a/b", "-fixture", ""] {
            let mut bad = binding.clone();
            bad.id = id.into();
            assert!(parse(&render(&bad), id).is_err());
        }
        let mut bad = binding.clone();
        bad.source.revision = "main".into();
        assert!(parse(&render(&bad), "fixture").is_err());
        bad = binding.clone();
        bad.source.repository = "https://user:secret@example.invalid/repo".into();
        assert!(parse(&render(&bad), "fixture").is_err());
        bad = binding.clone();
        bad.grants = vec![Grant::InteractiveBackend, Grant::InteractiveBackend];
        assert!(parse(&render(&bad), "fixture").is_err());
        doc.fields.get_mut("ahu").unwrap()["grants"] = serde_json::json!(["shell"]);
        assert!(parse(&doc.render().unwrap(), "fixture").is_err());
        let mut doc = crate::okf::Document::new("ahu:plugin-binding", &binding, "").unwrap();
        doc.fields.get_mut("ahu").unwrap()["executable"] = "command".into();
        assert!(parse(&doc.render().unwrap(), "fixture").is_err());
    }
    #[test]
    fn binding_files_are_context_and_symlinks_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join(DIRECTORY);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("fixture.md"), render(&binding())).unwrap();
        assert!(load(root.path(), "fixture").unwrap().is_some());
        assert!(
            crate::snapshot::collect(root.path())
                .unwrap()
                .entries
                .iter()
                .any(|e| e.path.ends_with("plugins/fixture.md"))
        );
        std::os::unix::fs::symlink("fixture.md", directory.join("link.md")).unwrap();
        assert!(load(root.path(), "link").is_err());
        assert!(load(root.path(), "../fixture").is_err());
    }
}
