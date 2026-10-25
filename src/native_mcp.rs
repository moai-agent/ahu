//! Private native configuration fingerprints supplied by installed runtimes.
//! No native path, format, or harness registry is built into core.
use crate::git::Repo;
use crate::util::{Error, Result};
use std::collections::BTreeMap;

pub(crate) fn fingerprints(repo: &Repo, harness: &str) -> Result<BTreeMap<String, String>> {
    let runtime = crate::harness_runtime::load(repo, harness)?;
    let home = crate::harness_runtime::native_home()?;
    let response=runtime.call("native_sources",serde_json::json!({"project":repo.root,"home":home,"settings":{"view":"private_fingerprints","environment_names":std::env::vars_os().filter_map(|(name,_)|name.into_string().ok()).collect::<Vec<_>>()}}))?;
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Response {
        fingerprints: BTreeMap<String, String>,
    }
    let response: Response = serde_json::from_value(response)
        .map_err(|_| Error::new("invalid native private fingerprints"))?;
    if response.fingerprints.len() > 64
        || response.fingerprints.iter().any(|(key, value)| {
            !key.starts_with(&format!("native:{}:", runtime.descriptor().id))
                || key.len() > 256
                || key.chars().any(char::is_control)
                || value.len() != 64
                || !value.bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        return Err(Error::new("invalid native private fingerprints"));
    }
    Ok(response.fingerprints)
}
