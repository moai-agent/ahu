//! Explicit native-global MCP setup. Paths are injected by the setup boundary;
//! tests never select a real home. Raw config bytes remain in memory only.
use crate::util::{Error, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Plan {
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Vec<u8>,
}

fn invalid() -> Error {
    Error::new(
        "Antigravity native MCP configuration is unsafe, unreadable, or changed; inspect native configuration before retrying",
    )
}

fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    crate::native_mcp::read_antigravity_config(path)
}

/// Prepare an ahu entry without changing any configuration. Refuse conflicts,
/// invalid JSON/JSONC and unsafe filesystem inputs without echoing their values.
/// The absolute executable is ahu itself; cwd is deliberately inherited from
/// each native session instead of pinning a particular repository globally.
pub fn plan(path: &Path, executable: &Path) -> Result<Plan> {
    if !executable.is_absolute() || !executable.is_file() {
        return Err(Error::new(
            "native MCP setup needs an absolute ahu executable",
        ));
    }
    let home = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(invalid)?;
    if !path.is_absolute()
        || path != home.join(".gemini/config/mcp_config.json")
        || path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(invalid());
    }
    let path = home
        .canonicalize()
        .map_err(|_| invalid())?
        .join(".gemini/config/mcp_config.json");
    let before = read(&path)?;
    let mut value: serde_json::Value = match &before {
        // Native `agy mcp add` accepts empty configuration files. Keep the
        // original bytes in `before` so publication still detects any edit.
        Some(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => serde_json::json!({}),
        Some(bytes) => serde_json::from_slice(bytes).map_err(|_| Error::new("Antigravity native MCP configuration must be valid JSON; existing content was not changed"))?,
        None => serde_json::json!({}),
    };
    let object = value.as_object_mut().ok_or_else(invalid)?;
    let servers = object
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(invalid)?;
    let wanted = serde_json::json!({"command":executable,"args":["mcp","serve"]});
    if let Some(existing) = servers.get("ahu") {
        // The native add command may include an empty env map. Preserve it.
        let mut normalized = existing.clone();
        if normalized.get("env") == Some(&serde_json::json!({})) {
            normalized.as_object_mut().unwrap().remove("env");
        }
        if normalized != wanted {
            return Err(Error::new(
                "Antigravity native MCP configuration already defines a different ahu server; setup will not replace it",
            ));
        }
        return Ok(Plan {
            path,
            after: before.clone().unwrap(),
            before,
        });
    }
    servers.insert("ahu".into(), wanted);
    let mut after = serde_json::to_vec_pretty(&value).map_err(|_| invalid())?;
    after.push(b'\n');
    // Pretty-printing can expand a compact input beyond the reader's bound.
    // Refuse before any publication rather than install an unreadable config.
    if after.len() > 1024 * 1024 {
        return Err(Error::new(
            "Antigravity native MCP configuration would exceed the supported size; existing content was not changed",
        ));
    }
    Ok(Plan {
        path,
        before,
        after,
    })
}

impl Plan {
    /// Recheck the planned input and publish atomically. Native tools do not
    /// share ahu's locks; concurrent native configuration writers must be stopped
    /// during setup. Changed inputs observed before publication are refused.
    pub fn apply(&self) -> Result<bool> {
        if read(&self.path)? != self.before {
            return Err(invalid());
        }
        if self.before.as_ref() == Some(&self.after) {
            return Ok(false);
        }
        let parent = self.path.parent().ok_or_else(invalid)?;
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|_| invalid())?;
        if read(&self.path)? != self.before {
            return Err(invalid());
        }
        if self.before.is_none() {
            if !crate::private_io::atomic_create(
                &self.path,
                &self.after,
                crate::private_io::Durability::Durable,
            )
            .map_err(|_| invalid())?
            {
                return Err(invalid());
            }
        } else {
            crate::private_io::atomic_write_with(
                &self.path,
                crate::private_io::Durability::Durable,
                |file| {
                    if read(&self.path)
                        .map_err(|_| std::io::Error::other("native configuration changed"))?
                        != self.before
                    {
                        return Err(std::io::Error::other("native configuration changed"));
                    }
                    file.write_all(&self.after)
                },
            )
            .map_err(|_| invalid())?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    const LIMIT: u64 = 1024 * 1024;

    #[test]
    fn native_setup_preserves_other_settings_is_cwd_sensitive_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join(".gemini/config/mcp_config.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = serde_json::json!({"extra":{"kept":true},"mcpServers":{"other":{"command":"synthetic","env":{"TOKEN":"synthetic-only"}}}});
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        let exe = Path::new("/usr/bin/true");
        assert!(plan(&path, exe).unwrap().apply().unwrap());
        let installed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(installed["extra"], original["extra"]);
        assert_eq!(
            installed["mcpServers"]["other"],
            original["mcpServers"]["other"]
        );
        assert_eq!(
            installed["mcpServers"]["ahu"],
            serde_json::json!({"command":exe,"args":["mcp","serve"]})
        );
        assert!(!plan(&path, exe).unwrap().apply().unwrap());
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn native_setup_creates_missing_config_and_refuses_conflicts_and_changed_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .canonicalize()
            .unwrap()
            .join(".gemini/config/mcp_config.json");
        let exe = Path::new("/usr/bin/true");
        assert!(plan(&path, exe).unwrap().apply().unwrap());
        for bytes in [
            b"{\"mcpServers\":{\"ahu\":{\"command\":\"different\"}}}".as_slice(),
            b"invalid synthetic json",
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(plan(&path, exe).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        std::fs::write(&path, b"{}").unwrap();
        let planned = plan(&path, exe).unwrap();
        std::fs::write(&path, b"{\"concurrent\":true}").unwrap();
        assert!(planned.apply().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"concurrent\":true}");
    }

    #[test]
    fn native_setup_accepts_empty_config_but_guards_its_original_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .canonicalize()
            .unwrap()
            .join(".gemini/config/mcp_config.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let exe = Path::new("/usr/bin/true");
        for bytes in [b"".as_slice(), b" \t\r\n"] {
            std::fs::write(&path, bytes).unwrap();
            let planned = plan(&path, exe).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert!(planned.apply().unwrap());
            let installed: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(
                installed["mcpServers"]["ahu"]["args"],
                serde_json::json!(["mcp", "serve"])
            );
            assert!(!plan(&path, exe).unwrap().apply().unwrap());

            std::fs::write(&path, bytes).unwrap();
            let planned = plan(&path, exe).unwrap();
            // Even another empty representation is a concurrent edit.
            std::fs::write(&path, b"\n\n").unwrap();
            assert!(planned.apply().is_err());
            assert_eq!(std::fs::read(&path).unwrap(), b"\n\n");
        }
        for bytes in [b"{ broken".as_slice(), b"\xff", b" \0 "] {
            std::fs::write(&path, bytes).unwrap();
            assert!(plan(&path, exe).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn native_setup_refuses_links_nonregular_oversized_and_writable_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let target = root.join(".gemini/config/mcp_config.json");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let external = root.join("external");
        std::fs::write(&external, b"{}").unwrap();
        symlink(&external, &target).unwrap();
        let exe = Path::new("/usr/bin/true");
        assert!(plan(&target, exe).is_err());
        assert!(plan(&root, exe).is_err());
        std::fs::remove_file(&target).unwrap();
        std::fs::write(&target, b"{}").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(plan(&target, exe).is_err());
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&target, vec![b' '; LIMIT as usize + 1]).unwrap();
        assert!(plan(&target, exe).is_err());
        std::fs::remove_file(&target).unwrap();
        std::fs::remove_dir(target.parent().unwrap()).unwrap();
        symlink(&root, target.parent().unwrap()).unwrap();
        assert!(plan(&target, exe).is_err());
    }

    #[test]
    fn native_setup_refuses_serialization_expansion_without_changing_input() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".gemini/config/mcp_config.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let before = serde_json::to_vec(&serde_json::json!({"extra": vec![0; 250_000]})).unwrap();
        assert!(before.len() < LIMIT as usize);
        std::fs::write(&path, &before).unwrap();
        assert!(plan(&path, Path::new("/usr/bin/true")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
