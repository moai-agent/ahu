//! Verified, local-only access to the skills shipped in the ahu binary.
//!
//! Retrieval is not evidence of harness loading or execution. The embedded
//! bundle is the allowlist; arbitrary repository or remote files are not skills
//! this server is authorized to expose.

use crate::util::{Error, Result, digest_bytes};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs::File;
use std::io::Read;
use std::path::Path;

const MAX_FILE_BYTES: usize = 64 * 1024;
const EXTENSION: &str = "io.modelcontextprotocol/skills";
const PAGE_SIZE: usize = 2;
const PROVENANCE: &str = "io.github.moai-agent.ahu/skillOrigin";
const MAX_CURSOR_BYTES: usize = 128;
const MAX_URI_BYTES: usize = 256;
const MAX_TOOL_PAYLOAD_BYTES: usize = 128 * 1024;

pub(super) fn tool_definitions() -> [Value; 2] {
    [
        json!({
            "name": "ahu_skills_list",
            "description": "List digest-verified, locally installed ahu-bundled skills with opaque pagination. Read-only discovery does not establish that a harness loaded or followed a skill.",
            "inputSchema": {"type":"object", "properties": {
                "cursor": {"type":"string", "minLength":1, "maxLength":MAX_CURSOR_BYTES}
            }, "additionalProperties":false},
            "annotations": {"readOnlyHint":true, "destructiveHint":false, "openWorldHint":false}
        }),
        json!({
            "name": "ahu_skills_get",
            "description": "Retrieve a verified ahu-bundled skill manifest and Markdown contents by exact URI. Reads only bundled SKILL.md files, never neighbors or remote content. Retrieval does not establish that a harness loaded or followed the skill.",
            "inputSchema": {"type":"object", "properties": {
                "uri": {"type":"string", "minLength":1, "maxLength":MAX_URI_BYTES,
                    "enum": super::BUNDLED_SKILLS.iter().map(|(name, _)| format!("ahu://skills/{name}/SKILL.md")).collect::<Vec<_>>()}
            }, "required":["uri"], "additionalProperties":false},
            "annotations": {"readOnlyHint":true, "destructiveHint":false, "openWorldHint":false}
        }),
    ]
}

pub(super) fn is_tool(name: &str) -> bool {
    matches!(name, "ahu_skills_list" | "ahu_skills_get")
}

pub(super) fn validate_tool_arguments(name: &str, arguments: &Value) -> Result<()> {
    let object = arguments
        .as_object()
        .ok_or_else(|| Error::new("skill tool arguments must be an object"))?;
    let listing = name == "ahu_skills_list";
    let key = if listing { "cursor" } else { "uri" };
    if object.keys().any(|candidate| candidate != key) {
        return Err(Error::new("unexpected skill tool argument"));
    }
    let Some(value) = object.get(key) else {
        return if listing {
            Ok(())
        } else {
            Err(Error::new("skill retrieval requires uri"))
        };
    };
    let limit = if listing {
        MAX_CURSOR_BYTES
    } else {
        MAX_URI_BYTES
    };
    let value = value
        .as_str()
        .filter(|text| !text.is_empty() && text.len() <= limit)
        .ok_or_else(|| Error::new("skill tool argument must be a bounded nonempty string"))?;
    if !listing
        && !super::BUNDLED_SKILLS
            .iter()
            .any(|(name, _)| value == format!("ahu://skills/{name}/SKILL.md"))
    {
        return Err(Error::new("unknown bundled skill URI"));
    }
    Ok(())
}

pub(super) fn call_tool(root: &Path, name: &str, arguments: &Value) -> Result<Value> {
    validate_tool_arguments(name, arguments)?;
    let method = if name == "ahu_skills_list" {
        "skills/list"
    } else {
        "skills/get"
    };
    let result = execute_with_content(root, method, arguments, true)?;
    // Bound the serialized payload too, before MCP duplicates it as text and
    // structured content. This keeps the complete response below the frame cap.
    if serde_json::to_vec(&result)?.len() > MAX_TOOL_PAYLOAD_BYTES {
        return Err(Error::new("skill tool result exceeds 128 KiB"));
    }
    Ok(result)
}

#[derive(Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
}

struct VerifiedSkill {
    name: String,
    description: String,
    digest: String,
    text: String,
    frontmatter: Value,
}

fn frontmatter(text: &str, expected_name: &str) -> Result<(Frontmatter, Value)> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .ok_or_else(|| Error::new("skill requires YAML frontmatter"))?;
    let mut end = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let front: Frontmatter = yaml_serde::from_str(&rest[..end])
                .map_err(|_| Error::new("invalid skill frontmatter"))?;
            if front.name != expected_name
                || front.name.is_empty()
                || front.name.len() > 64
                || front.name.starts_with('-')
                || front.name.ends_with('-')
                || front.name.contains("--")
                || !front
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                || front.description.trim().is_empty()
                || front.description.len() > 1024
            {
                return Err(Error::new("invalid skill name or description"));
            }
            let object: Value = yaml_serde::from_str(&rest[..end])
                .map_err(|_| Error::new("skill frontmatter must be a JSON-compatible object"))?;
            return Ok((front, object));
        }
        end += line.len();
    }
    Err(Error::new("skill frontmatter is not closed"))
}

// All components are opened relative to pinned descriptors. Never follow a
// symlink, even if it points back inside the checkout; never block on a FIFO.
fn open_child(parent: &File, name: &str, directory: bool) -> Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(Error::new("invalid skill path component"));
    }
    let name = std::ffi::CString::new(name).map_err(|_| Error::new("invalid skill path"))?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    // SAFETY: parent is live, name is NUL-terminated, no creation mode is needed.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(Error::new(
            "skill file is missing, inaccessible, or a symlink",
        ));
    }
    // SAFETY: this fresh descriptor is transferred exactly once to File.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn read_verified(root: &Path, name: &str, expected: &str) -> Result<VerifiedSkill> {
    use std::os::unix::fs::OpenOptionsExt;
    // Validate the trusted bundle too, before advertising any metadata.
    if expected.len() > MAX_FILE_BYTES {
        return Err(Error::new("skill exceeds 64 KiB"));
    }
    let (front, frontmatter) = frontmatter(expected, name)?;
    let root = File::options()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let agents = open_child(&root, ".agents", true)?;
    let skills = open_child(&agents, "skills", true)?;
    let directory = open_child(&skills, name, true)?;
    let file = open_child(&directory, "SKILL.md", false)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES as u64 {
        return Err(Error::new("skill must be a regular file at most 64 KiB"));
    }
    let mut bytes = Vec::new();
    file.take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    let digest = digest_bytes(&bytes);
    if bytes.len() > MAX_FILE_BYTES || digest != digest_bytes(expected.as_bytes()) {
        return Err(Error::new("skill content differs from the ahu bundle"));
    }
    // Return exactly the validated bytes, never reopen a path after validation.
    let text = String::from_utf8(bytes).map_err(|_| Error::new("skill must be UTF-8"))?;
    Ok(VerifiedSkill {
        name: front.name,
        description: front.description,
        digest,
        text,
        frontmatter,
    })
}

impl VerifiedSkill {
    fn uri(&self) -> String {
        format!("ahu://skills/{}/SKILL.md", self.name)
    }

    fn resource(&self) -> Value {
        json!({
            "uri": self.uri(), "name": self.name,
            "description": self.description, "mimeType": "text/markdown",
            "digest": format!("sha256:{}", self.digest), "size": self.text.len(),
        })
    }

    fn manifest(&self) -> Value {
        json!({
            "uri": self.uri(), "frontmatter": self.frontmatter,
            // The existing distributed bundle contains SKILL.md only. Never
            // enumerate neighbors or follow links in Markdown to extend it.
            "resources": [self.resource()],
            "_meta": {PROVENANCE: {
                "kind": "ahu-bundled-local", "serverVersion": super::SERVER_VERSION,
                "path": super::skill_path(&self.name),
                "digest": format!("sha256:{}", self.digest),
                "executionObserved": false,
            }},
        })
    }
}

fn catalog(root: &Path) -> Vec<VerifiedSkill> {
    super::BUNDLED_SKILLS
        .iter()
        .filter_map(|(name, text)| read_verified(root, name, text).ok())
        .collect()
}

pub(super) fn capabilities(root: &Path) -> Value {
    let mut capabilities = json!({"tools": {}, "extensions": {super::TASKS_EXTENSION: {}}});
    if !catalog(root).is_empty() {
        capabilities["resources"] = json!({});
        capabilities["extensions"][EXTENSION] = json!({});
    }
    capabilities
}

pub(super) fn handle(
    root: &Path,
    id: &Value,
    method: &str,
    params: &Value,
    modern: bool,
) -> Option<Value> {
    if !matches!(
        method,
        "skills/list" | "skills/get" | "resources/list" | "resources/read"
    ) {
        return None;
    }
    if !modern {
        return Some(super::rpc_error(id, -32601, "method requires modern MCP"));
    }
    if method.starts_with("skills/")
        && !params["_meta"][super::CLIENT_CAPABILITIES_META]["extensions"][EXTENSION].is_object()
    {
        let mut error = super::rpc_error(id, -32021, "MCP Skills extension required");
        error["error"]["data"] = json!({"requiredCapabilities": {"extensions": {EXTENSION: {}}}});
        return Some(error);
    }
    Some(match execute(root, method, params) {
        Ok(result) => super::response(id, result),
        Err(error) => super::rpc_error(id, -32602, error.to_string()),
    })
}

fn execute(root: &Path, method: &str, params: &Value) -> Result<Value> {
    execute_with_content(root, method, params, false)
}

fn execute_with_content(
    root: &Path,
    method: &str,
    params: &Value,
    include_content: bool,
) -> Result<Value> {
    let listing = matches!(method, "skills/list" | "resources/list");
    let argument = if listing { "cursor" } else { "uri" };
    if !params.is_object()
        || params
            .as_object()
            .unwrap()
            .keys()
            .any(|key| key != "_meta" && key != argument)
    {
        return Err(Error::new("unexpected skill/resource request parameters"));
    }
    let skills = catalog(root);
    if listing {
        // Bind opaque continuation cursors to the full verified inventory.
        // A changed/missing file invalidates a previously issued cursor.
        let manifests: Vec<_> = skills.iter().map(VerifiedSkill::manifest).collect();
        let fingerprint = digest_bytes(&serde_json::to_vec(&manifests)?);
        let start = match params.get("cursor") {
            None => 0,
            Some(Value::String(cursor)) => {
                let (digest, offset) = cursor
                    .split_once(':')
                    .ok_or_else(|| Error::new("invalid skills cursor"))?;
                let offset = offset
                    .parse::<usize>()
                    .map_err(|_| Error::new("invalid skills cursor"))?;
                if digest != fingerprint
                    || offset == 0
                    || offset >= skills.len()
                    || offset % PAGE_SIZE != 0
                {
                    return Err(Error::new("invalid or stale skills cursor"));
                }
                offset
            }
            _ => return Err(Error::new("cursor must be a string")),
        };
        let end = (start + PAGE_SIZE).min(skills.len());
        let mut result = json!({"resultType": "complete", "ttlMs": 0, "cacheScope": "private"});
        if method == "skills/list" {
            result["skills"] = json!(manifests[start..end]);
        } else {
            result["resources"] = json!(
                skills[start..end]
                    .iter()
                    .map(VerifiedSkill::resource)
                    .collect::<Vec<_>>()
            );
        }
        if end < skills.len() {
            result["nextCursor"] = json!(format!("{fingerprint}:{end}"));
        }
        return Ok(result);
    }
    let uri = params["uri"]
        .as_str()
        .ok_or_else(|| Error::new("uri must be a string"))?;
    // Exact allowlist lookup: no URI decoding, paths, remote retrieval, or
    // client-controlled origins ever become filesystem inputs.
    let skill = skills
        .iter()
        .find(|skill| skill.uri() == uri)
        .ok_or_else(|| Error::new("unknown or unverified skill resource"))?;
    if method == "skills/get" {
        let mut result = json!({"resultType": "complete", "skill": skill.manifest(), "ttlMs": 0, "cacheScope": "private"});
        if include_content {
            result["contents"] =
                json!([{"uri":skill.uri(), "mimeType":"text/markdown", "text":skill.text}]);
        }
        Ok(result)
    } else {
        Ok(
            json!({"resultType": "complete", "contents": [{"uri": skill.uri(), "mimeType": "text/markdown", "text": skill.text}]}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL: &str =
        "---\nname: example\ndescription: Synthetic local skill.\n---\nInstructions.\n";

    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".agents/skills/example")).unwrap();
        std::fs::write(root.path().join(".agents/skills/example/SKILL.md"), SKILL).unwrap();
        root
    }

    fn bundled_fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for (name, text) in super::super::BUNDLED_SKILLS {
            let directory = root.path().join(".agents/skills").join(name);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("SKILL.md"), text).unwrap();
        }
        root
    }

    #[test]
    fn skill_tool_arguments_are_exact_bounded_and_allowlisted() {
        let uri = format!(
            "ahu://skills/{}/SKILL.md",
            super::super::BUNDLED_SKILLS[0].0
        );
        assert!(validate_tool_arguments("ahu_skills_list", &json!({})).is_ok());
        assert!(validate_tool_arguments("ahu_skills_list", &json!({"cursor":"digest:2"})).is_ok());
        assert!(validate_tool_arguments("ahu_skills_get", &json!({"uri":uri})).is_ok());
        for (name, arguments) in [
            ("ahu_skills_list", json!([])),
            ("ahu_skills_list", json!({"extra":true})),
            ("ahu_skills_list", json!({"cursor":7})),
            (
                "ahu_skills_list",
                json!({"cursor":"x".repeat(MAX_CURSOR_BYTES + 1)}),
            ),
            ("ahu_skills_get", json!({})),
            (
                "ahu_skills_get",
                json!({"uri":"ahu://skills/other/SKILL.md"}),
            ),
            (
                "ahu_skills_get",
                json!({"uri":"x".repeat(MAX_URI_BYTES + 1)}),
            ),
        ] {
            assert!(validate_tool_arguments(name, &arguments).is_err());
        }
    }

    #[test]
    fn capabilities_and_handlers_gate_skill_access_and_read_verified_resources() {
        let empty = tempfile::tempdir().unwrap();
        let absent = capabilities(empty.path());
        assert!(absent.get("resources").is_none());
        assert!(absent["extensions"].get(EXTENSION).is_none());

        let root = bundled_fixture();
        let advertised = capabilities(root.path());
        assert_eq!(advertised["extensions"][EXTENSION], json!({}));
        assert_eq!(advertised["resources"], json!({}));

        let id = json!(7);
        assert!(handle(root.path(), &id, "unrelated/method", &json!({}), true).is_none());
        let legacy = handle(root.path(), &id, "skills/list", &json!({}), false).unwrap();
        assert_eq!(legacy["error"]["code"], -32601);
        let missing = handle(root.path(), &id, "skills/list", &json!({}), true).unwrap();
        assert_eq!(missing["error"]["code"], -32021);
        assert_eq!(
            missing["error"]["data"]["requiredCapabilities"]["extensions"][EXTENSION],
            json!({})
        );

        let params = json!({
            "_meta": {"io.modelcontextprotocol/clientCapabilities": {
                "extensions": {EXTENSION: {}}
            }}
        });
        let listed = handle(root.path(), &id, "skills/list", &params, true).unwrap();
        assert_eq!(listed["result"]["resultType"], "complete");
        assert_eq!(
            listed["result"]["skills"].as_array().unwrap().len(),
            PAGE_SIZE
        );
        let cursor = listed["result"]["nextCursor"].as_str().unwrap();
        let second_page = execute(root.path(), "skills/list", &json!({"cursor":cursor})).unwrap();
        assert!(!second_page["skills"].as_array().unwrap().is_empty());

        let name = super::super::BUNDLED_SKILLS[0].0;
        let uri = format!("ahu://skills/{name}/SKILL.md");
        let manifest = handle(
            root.path(),
            &id,
            "skills/get",
            &json!({"uri":uri,"_meta":params["_meta"]}),
            true,
        )
        .unwrap();
        assert_eq!(manifest["result"]["skill"]["uri"], uri);
        assert!(manifest["result"].get("contents").is_none());
        let tool_result = call_tool(root.path(), "ahu_skills_get", &json!({"uri":uri})).unwrap();
        assert!(tool_result["contents"].is_array());
        let resource = handle(
            root.path(),
            &id,
            "resources/read",
            &json!({"uri":uri}),
            true,
        )
        .unwrap();
        assert_eq!(resource["result"]["contents"][0]["uri"], uri);
        let invalid = handle(
            root.path(),
            &id,
            "resources/read",
            &json!({"uri":"ahu://skills/other/SKILL.md"}),
            true,
        )
        .unwrap();
        assert_eq!(invalid["error"]["code"], -32602);
    }

    #[test]
    fn verifies_exact_bytes_and_metadata() {
        let root = fixture();
        let skill = read_verified(root.path(), "example", SKILL).unwrap();
        assert_eq!(skill.name, "example");
        assert_eq!(skill.description, "Synthetic local skill.");
        assert_eq!(skill.digest, digest_bytes(SKILL.as_bytes()));
        assert_eq!(skill.text, SKILL);
    }

    #[test]
    fn preserves_optional_frontmatter_and_accepts_the_exact_size_limit() {
        let text = SKILL.replace(
            "description: Synthetic local skill.",
            "description: Synthetic local skill.\nmetadata:\n  author: fixture\nallowed-tools: Read",
        );
        let (_, object) = frontmatter(&text, "example").unwrap();
        assert_eq!(object["metadata"]["author"], "fixture");
        assert_eq!(object["allowed-tools"], "Read");
        let text = format!("{text}{}", "x".repeat(MAX_FILE_BYTES - text.len()));
        let root = fixture();
        std::fs::write(root.path().join(".agents/skills/example/SKILL.md"), &text).unwrap();
        assert_eq!(
            read_verified(root.path(), "example", &text)
                .unwrap()
                .text
                .len(),
            MAX_FILE_BYTES
        );
    }

    #[test]
    fn rejects_fifo_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        let root = fixture();
        let path = root.path().join(".agents/skills/example/SKILL.md");
        std::fs::remove_file(&path).unwrap();
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: path is NUL-terminated and names a missing temporary fixture.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(read_verified(root.path(), "example", SKILL).is_err());
    }

    #[test]
    fn rejects_malformed_bundle_metadata() {
        for invalid in [
            "name: example\ndescription: missing delimiters",
            "---\nname: example\n---\nmissing description",
            "---\nname: other\ndescription: wrong name\n---\n",
            "---\nname: example\nname: example\ndescription: duplicate\n---\n",
            "---\nname: example\ndescription: ''\n---\n",
            "---\nname: example\ndescription: unclosed\n",
        ] {
            assert!(frontmatter(invalid, "example").is_err());
        }
        assert!(frontmatter(&SKILL.replace('\n', "\r\n"), "example").is_ok());
        for &(name, text) in super::super::BUNDLED_SKILLS {
            assert!(frontmatter(text, name).is_ok(), "{name}");
        }
    }

    #[test]
    fn rejects_changed_missing_oversized_and_nonregular_content() {
        let root = fixture();
        let path = root.path().join(".agents/skills/example/SKILL.md");
        for bytes in [
            b"changed".to_vec(),
            vec![0xff],
            vec![b'x'; MAX_FILE_BYTES + 1],
        ] {
            std::fs::write(&path, bytes).unwrap();
            assert!(read_verified(root.path(), "example", SKILL).is_err());
        }
        std::fs::remove_file(&path).unwrap();
        assert!(read_verified(root.path(), "example", SKILL).is_err());
        std::fs::create_dir(&path).unwrap();
        assert!(read_verified(root.path(), "example", SKILL).is_err());
    }

    #[test]
    fn rejects_symlinks_at_every_component_and_path_escapes() {
        use std::os::unix::fs::symlink;
        for relative in [
            ".agents",
            ".agents/skills",
            ".agents/skills/example",
            ".agents/skills/example/SKILL.md",
        ] {
            let root = fixture();
            let path = root.path().join(relative);
            let saved = root.path().join("saved");
            std::fs::rename(&path, &saved).unwrap();
            symlink(&saved, &path).unwrap();
            assert!(
                read_verified(root.path(), "example", SKILL).is_err(),
                "{relative}"
            );
        }
        let root = fixture();
        let directory = File::open(root.path()).unwrap();
        for invalid in [
            "",
            ".",
            "..",
            "../example",
            "/example",
            "a/b",
            "a\\b",
            "a\0b",
        ] {
            assert!(open_child(&directory, invalid, false).is_err());
        }
    }
}
