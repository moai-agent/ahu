//! User-facing task references. Stored task identifiers remain unchanged.

use crate::util::Result;

pub fn resolve(repo: &crate::git::Repo, input: &str) -> Result<String> {
    if input.starts_with('@') {
        crate::task_handles::resolve(repo, input)
    } else {
        normalize(input)
    }
}

/// Accept the existing bare identifier/prefix or its explicit task reference.
/// Never reinterpret a reference to another resource kind as a task.
pub fn normalize(input: &str) -> Result<String> {
    let lower = input.to_ascii_lowercase();
    let id = if let Some(reference) = lower.strip_prefix("ahu:") {
        reference.strip_prefix("task:").ok_or_else(|| {
            crate::util::Error::new("expected a task reference: ahu:task:<id>")
                .with_kind(crate::util::ErrorKind::Usage)
        })?
    } else {
        &lower
    };
    if id.is_empty() {
        crate::bail!(kind: crate::util::ErrorKind::Usage, "a task id must not be empty.");
    }
    if id.starts_with('@') {
        crate::bail!(kind: crate::util::ErrorKind::Usage, "use @name directly for a task handle, or ahu:task:<uuid> for a canonical reference");
    }
    Ok(id.to_string())
}

pub fn display(id: &str) -> String {
    format!("ahu:task:{id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_preserve_legacy_prefixes_and_refuse_other_resource_kinds() {
        for input in ["ABC123", "ahu:task:ABC123", "AHU:TASK:abc123"] {
            assert_eq!(normalize(input).unwrap(), "abc123");
        }
        for input in ["", "ahu:task:", "ahu:agent:abc123", "ahu:unknown:abc123"] {
            assert!(normalize(input).is_err());
        }
    }
}
