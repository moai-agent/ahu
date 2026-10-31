//! Owned installed suggestions and explicit native model observations.
use crate::git::Repo;
use crate::harness_runtime::{Descriptor, Operation};
use crate::util::{Error, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Native,
    Suggested,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub models: Vec<String>,
    pub source: Source,
    pub entitlement_checked: bool,
}

/// Pure metadata: does not probe a provider or claim account entitlement.
pub fn suggestions(descriptor: &Descriptor) -> Options {
    Options {
        models: descriptor.models.clone(),
        source: Source::Suggested,
        entitlement_checked: false,
    }
}

/// Explicit inspection through the installed runtime. Failures are never hidden
/// by substituting a static catalog or metadata suggestions.
pub fn for_harness(repo: &Repo, harness: &str) -> Result<Options> {
    let runtime = crate::harness_runtime::load(repo, harness)?;
    if !runtime.descriptor().operations.contains(&Operation::Models) {
        return Ok(suggestions(runtime.descriptor()));
    }
    let before = crate::snapshot::collect(&repo.root)?.digest();
    let value = runtime.call(
        "models",
        crate::harness_runtime::native_params(repo, runtime.descriptor())?,
    );
    if crate::snapshot::collect(&repo.root)?.digest() != before {
        return Err(Error::new(
            "native model inspection changed project context; review before retrying",
        ));
    }
    let value = value?;
    let options: Options =
        serde_json::from_value(value).map_err(|_| Error::new("invalid runtime model response"))?;
    validate(&options)?;
    Ok(options)
}
fn validate(options: &Options) -> Result<()> {
    if options.models.len() > 4096
        || options.models.iter().any(|id| !valid_id(id))
        || options
            .models
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != options.models.len()
        || (options.source == Source::Suggested && options.entitlement_checked)
    {
        return Err(Error::new(
            "invalid runtime model suggestions or observations",
        ));
    }
    Ok(())
}
/// Exact model identities are opaque to core, but never flags or control text.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 1024
        && !id.starts_with('-')
        && !id.chars().any(|c| c.is_control() || c.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opaque_exact_models_do_not_depend_on_a_provider_catalog() {
        for id in [
            "invented-sixth/v42",
            "new-exact-model",
            "vendor:model+build",
        ] {
            assert!(valid_id(id));
        }
        for id in ["", "--flag", "space model", "model\nextra"] {
            assert!(!valid_id(id));
        }
        let options = Options {
            models: vec!["invented-sixth/v42".into()],
            source: Source::Native,
            entitlement_checked: false,
        };
        validate(&options).unwrap();
        let duplicate = Options {
            models: vec!["same".into(), "same".into()],
            ..options
        };
        assert!(validate(&duplicate).is_err());
        let false_claim = Options {
            models: vec![],
            source: Source::Suggested,
            entitlement_checked: true,
        };
        assert!(validate(&false_claim).is_err());
    }
}
