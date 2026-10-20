//! Standalone executable boundary; the compiled adapter remains unchanged.
mod server;

use ahu_cmux_plugin::Cmux;
use ahu_host_api::{Error, Result};
use std::path::{Path, PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("ahu-cmux-host: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    if std::env::args_os().skip(1).collect::<Vec<_>>() != ["--ahu-host-protocol=1"] {
        return Err(Error::new("usage: ahu-cmux-host --ahu-host-protocol=1"));
    }
    server::serve(
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
        || Cmux::connect(resolve_cmux()?),
    )
}

fn resolve_cmux() -> Result<PathBuf> {
    // A standalone caller may explicitly choose a CLI. Core deliberately does
    // not forward this variable through its executable-host environment.
    if let Some(path) = std::env::var_os("AHU_CMUX_BIN") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(Error::new(
                "standalone AHU_CMUX_BIN must be an absolute path",
            ));
        }
        return Ok(path);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let candidates = std::env::split_paths(&path)
        .filter(|p| p.is_absolute())
        .map(|p| p.join("cmux"))
        .chain([PathBuf::from(
            "/Applications/cmux.app/Contents/Resources/bin/cmux",
        )]);
    candidates.filter_map(|p| implicit_candidate(&p)).next().ok_or_else(|| {
        Error::new("cmux CLI unavailable outside Git working trees; install cmux or set an absolute AHU_CMUX_BIN for standalone use")
    })
}

fn implicit_candidate(path: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = path.canonicalize().ok()?;
    let metadata = path.metadata().ok()?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return None;
    }
    // Match core's implicit utility boundary without invoking candidate Git or
    // executing a repository-controlled CLI during lookup.
    for (depth, ancestor) in path.parent()?.ancestors().enumerate() {
        if depth >= 256 {
            return None;
        }
        match std::fs::symlink_metadata(ancestor.join(".git")) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return None,
        }
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn implicit_lookup_rejects_nonexecutables_and_repository_targets() {
        let temp = tempfile::tempdir().unwrap();
        let cli = temp.path().join("cmux");
        std::fs::write(&cli, "#!/bin/sh\nexit 0\n").unwrap();
        assert!(implicit_candidate(&cli).is_none());
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(implicit_candidate(&cli), Some(cli.canonicalize().unwrap()));
        let link = temp.path().join("alias");
        symlink(&cli, &link).unwrap();
        std::fs::write(temp.path().join(".git"), "gitdir: ignored").unwrap();
        assert!(implicit_candidate(&cli).is_none());
        assert!(implicit_candidate(&link).is_none());
    }
}
