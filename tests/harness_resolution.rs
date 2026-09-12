//! Where a harness binary is allowed to come from.
//!
//! ahu resolves a harness in order to run it, so resolution is the point at
//! which a repository could hand ahu something to execute. Two rules hold, and
//! they hold for *every* invocation — the launch, the `run-task` exec, and the
//! `--version` probes that run while a preview is still being built:
//!
//!   - a `PATH` entry that is empty or relative resolves against the current
//!     directory, which for `ahu run-task` is a checkout of the repository, so
//!     it is skipped rather than followed;
//!   - a binary inside a repository ahu has opened is refused even when an
//!     absolute `PATH` entry points at it.
//!
//! These run ahu as a child process. Resolution reads the process environment,
//! and a preview is printed before any confirmation, so the only faithful
//! reproduction is a whole command with a hostile `PATH`.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::TestRepo;

/// A repository that registers one agent and commits a `claude` of its own.
///
/// The planted binary records the fact that it ran. Nothing about it is
/// malicious: the finding is that it runs at all, before the user has been shown
/// anything to approve.
struct PlantedRepo {
    repo: TestRepo,
    marker: PathBuf,
    external_bin: PathBuf,
    outside: tempfile::TempDir,
}

fn planted(where_in_repo: &str) -> PlantedRepo {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("sable", "1.0.0", "claude-sonnet-5");
    repo.write("assignment.txt", "review the launcher\n");

    let outside = tempfile::TempDir::new().expect("outside dir");
    let marker = outside.path().join("repo-binary-ran");
    let planted = repo.write(
        where_in_repo,
        &format!(
            "#!/bin/sh\n: > '{marker}'\nprintf 'claude 2.1.269\\n'\nexit 0\n",
            marker = marker.display()
        ),
    );
    make_executable(&planted);
    repo.commit("fixture");

    // A legitimate installation, outside the repository, later on PATH.
    let external_bin = common::fake_harness(outside.path(), &outside.path().join("argv"));
    PlantedRepo {
        repo,
        marker,
        external_bin,
        outside,
    }
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn run(fixture: &PlantedRepo, path: &str, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ahu"))
        .args(args)
        .current_dir(fixture.repo.path())
        .env("AHU_STATE_DIR", fixture.repo.state_path())
        .env("AHU_CMUX_BIN", fixture.outside.path().join("missing-cmux"))
        // A throwaway home, so the developer's own settings cannot change this.
        .env("HOME", fixture.outside.path())
        .env("PATH", path)
        .output()
        .expect("ahu runs")
}

/// A relative `PATH` entry must never execute a repository binary — not even to
/// ask it for its version.
///
/// `which` already skipped relative entries when *launching*, but the
/// prerequisite and enforcement probes still called `Command::new("claude")`,
/// which repeats the operating system's own PATH lookup and honours `.`. A
/// `--version` request is arbitrary code execution, and it happened before the
/// preview the user is supposed to approve was printed.
#[test]
fn a_relative_path_entry_never_runs_a_repository_binary_even_for_a_version_probe() {
    let fixture = planted("claude");
    let path = format!(".:{}:/usr/bin:/bin", fixture.external_bin.display());

    let output = run(
        &fixture,
        &path,
        &[
            "launch",
            "@sable",
            "--prompt-file",
            "assignment.txt",
            "--dry-run",
        ],
    );

    assert!(
        !fixture.marker.exists(),
        "the repository's own claude was executed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "the legitimate installation should still resolve: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains(
            &fixture
                .external_bin
                .join("claude")
                .to_string_lossy()
                .to_string()
        ),
        "the preview must name the external binary: {text}"
    );
}

/// The same for an absolute `PATH` entry that points into the checkout.
///
/// This one passed the relative-entry filter entirely. `run_task` refused it at
/// exec time, but the probes had already run it.
#[test]
fn an_absolute_path_entry_inside_the_repository_is_refused() {
    let fixture = planted("tools/claude");
    let path = format!(
        "{}:{}:/usr/bin:/bin",
        fixture.repo.path().join("tools").display(),
        fixture.external_bin.display()
    );

    let output = run(
        &fixture,
        &path,
        &[
            "launch",
            "@sable",
            "--prompt-file",
            "assignment.txt",
            "--dry-run",
        ],
    );

    assert!(
        !fixture.marker.exists(),
        "a harness was executed from inside the repository: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        !text.contains(
            &fixture
                .repo
                .path()
                .join("tools/claude")
                .to_string_lossy()
                .to_string()
        ),
        "the repository's binary must not be the resolved harness: {text}"
    );
}

/// `ahu doctor` probes every harness in the catalog, so it is the widest
/// version-probe surface ahu has.
#[test]
fn doctor_probes_no_repository_binary() {
    let fixture = planted("claude");
    let path = format!(
        ".:{}:{}:/usr/bin:/bin",
        fixture.repo.path().display(),
        fixture.external_bin.display()
    );

    let output = run(&fixture, &path, &["doctor"]);

    assert!(
        !fixture.marker.exists(),
        "doctor executed the repository's claude: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// With no legitimate installation, a repository binary is a missing harness —
/// never a found one.
#[test]
fn a_repository_binary_alone_is_reported_as_no_harness_at_all() {
    let fixture = planted("claude");
    let path = format!(".:{}:/usr/bin:/bin", fixture.repo.path().display());

    let output = run(
        &fixture,
        &path,
        &[
            "launch",
            "@sable",
            "--prompt-file",
            "assignment.txt",
            "--dry-run",
        ],
    );

    assert!(!fixture.marker.exists());
    assert!(
        !output.status.success(),
        "ahu must refuse rather than fall back to the repository's binary"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not installed on this machine"), "{stderr}");
}

/// A program name is a name, not a path.
///
/// `PATH` lookup is the only way a harness is resolved; a name carrying a
/// separator would be resolved against the current directory instead.
#[test]
fn a_program_name_containing_a_separator_is_not_resolved() {
    assert!(ahu::selection::resolve_executable("./claude").is_none());
    assert!(ahu::selection::resolve_executable("../claude").is_none());
    assert!(ahu::selection::resolve_executable("tools/claude").is_none());
    assert!(ahu::selection::resolve_executable("").is_none());
}

/// The exclusion is a property of the resolver, not of one call site.
#[test]
fn a_registered_repository_excludes_binaries_under_it() {
    let repo = TestRepo::new();
    repo.commit("fixture");
    // `discover` is where a repository enters the process, and where it is
    // registered, so every later resolution and probe is covered by one call.
    let discovered = ahu::git::discover(repo.path()).unwrap();

    assert!(ahu::selection::is_excluded(&discovered.root.join("claude")));
    assert!(ahu::selection::is_excluded(
        &discovered.root.join("tools/nested/claude")
    ));
    assert!(ahu::selection::is_excluded(
        &discovered.root.join(".worktrees/abc/claude")
    ));

    let outside = tempfile::TempDir::new().unwrap();
    assert!(!ahu::selection::is_excluded(&outside.path().join("claude")));

    // A probe of an excluded path is refused rather than run.
    assert!(
        ahu::selection::probe_version(&discovered.root.join("claude").to_string_lossy()).is_none()
    );
    // And so is a relative one.
    assert!(ahu::selection::probe_version("claude").is_none());
}
