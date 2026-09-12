//! Shared fixtures: throwaway Git repositories and a fake harness executable.
//!
//! Every test that touches the filesystem works inside a `tempfile::TempDir` and
//! points `AHU_STATE_DIR` at it, so no test can read or write a developer's real
//! ahu state, cmux session, or checkouts.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

pub struct TestRepo {
    pub dir: TempDir,
    pub state: TempDir,
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "ahu tests")
        .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
        .env("GIT_COMMITTER_NAME", "ahu tests")
        .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
        // Ignore the developer's own global/system Git config so a personal
        // `core.excludesFile` cannot change what these fixtures commit.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

impl TestRepo {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("temp dir");
        let state = TempDir::new().expect("state dir");
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.name", "ahu tests"]);
        git(
            dir.path(),
            &["config", "user.email", "tests@example.invalid"],
        );
        let repo = TestRepo { dir, state };
        repo.write("README.md", "# fixture\n");
        repo.commit("initial");
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.dir.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&path, contents).expect("write fixture file");
        path
    }

    pub fn remove(&self, relative: &str) {
        std::fs::remove_file(self.dir.path().join(relative)).expect("remove fixture file");
    }

    pub fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(relative)).expect("read fixture file")
    }

    pub fn commit(&self, message: &str) {
        // `-f` because agent configuration directories such as `.claude/` are
        // commonly ignored globally; these fixtures commit them deliberately.
        git(self.dir.path(), &["add", "-A", "-f"]);
        git(
            self.dir.path(),
            &["commit", "-q", "--allow-empty", "-m", message],
        );
    }

    /// A minimal, valid project configuration.
    pub fn init_config(&self) {
        self.write(
            ".agents/ahu/config.toml",
            &format!(
                "schema_version = 1\n\
                 harness_preferences = [\"claude-code\"]\n\
                 model_selection = \"project-ranked\"\n\
                 catalog_version = \"{}\"\n\
                 \n[model_rankings]\n\
                 \"claude-code\" = [\"claude-opus-5\", \"claude-sonnet-5\"]\n\
                 \n[context_hygiene]\n\
                 review_on_first_load = true\n\
                 review_interval_days = 7\n",
                ahu::catalog::CATALOG_VERSION
            ),
        );
    }

    /// A registered Claude Code agent backed by a native definition.
    pub fn add_agent(&self, name: &str, version: &str, model: &str) {
        self.write(
            &format!(".claude/agents/{name}.md"),
            &format!(
                "---\nname: {name}\ndescription: fixture agent\nmodel: {model}\ntools: Read, Edit\n---\n\nYou are {name}.\n"
            ),
        );
        self.write(
            &format!(".agents/ahu/agents/{name}.toml"),
            &format!(
                "schema_version = 1\n\
                 name = \"{name}\"\n\
                 version = \"{version}\"\n\
                 description = \"fixture agent\"\n\
                 harness = \"claude-code\"\n\
                 model = \"{model}\"\n\
                 \n[source]\n\
                 format = \"claude-agent\"\n\
                 path = \".claude/agents/{name}.md\"\n"
            ),
        );
    }

    /// A registered agent pinned to an arbitrary harness and model.
    ///
    /// `add_agent` always writes `harness = "claude-code"`. A launch test that
    /// has to prove two agents keep *different* configured harnesses needs a
    /// second manifest that names another one, with an instructions file the
    /// non-Claude source format accepts.
    pub fn add_agent_on(&self, name: &str, version: &str, harness: &str, model: &str) {
        self.write(
            &format!(".agents/ahu/instructions/{name}.md"),
            &format!("You are {name}. Fixture instructions.\n"),
        );
        self.write(
            &format!(".agents/ahu/agents/{name}.toml"),
            &format!(
                "schema_version = 1\n\
                 name = \"{name}\"\n\
                 version = \"{version}\"\n\
                 description = \"fixture agent\"\n\
                 harness = \"{harness}\"\n\
                 model = \"{model}\"\n\
                 \n[source]\n\
                 format = \"markdown\"\n\
                 path = \".agents/ahu/instructions/{name}.md\"\n"
            ),
        );
    }

    pub fn state_path(&self) -> &Path {
        self.state.path()
    }
}

/// Install a fake `claude` executable that records its argv and stdin-free
/// environment, so a test can assert exactly what the harness received.
pub fn fake_harness(dir: &Path, record: &Path) -> PathBuf {
    fake_harnesses(dir, &["claude"], |_| record.to_path_buf())
}

/// Install a fake executable per harness name, each recording its own argv.
///
/// `record_for` chooses the file a given harness writes to, so a test that
/// launches two agents on two harnesses can tell the two argv dumps apart —
/// which is the only way to prove each one ran under its *own* harness rather
/// than both under whichever binary PATH happened to resolve first.
pub fn fake_harnesses(
    dir: &Path,
    programs: &[&str],
    record_for: impl Fn(&str) -> PathBuf,
) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("create bin dir");
    for program in programs {
        let record = record_for(program);
        let script = bin.join(program);
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 : > '{record}'\n\
                 for arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{record}'; done\n\
                 exit 0\n",
                record = record.display()
            ),
        )
        .expect("write fake harness");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod fake harness");
        }
    }
    bin
}

/// A prompt built to break anything that treats it as shell input.
pub const HOSTILE_PROMPT: &str = "Fix $(touch /tmp/ahu-pwned) and `rm -rf /` now\n\
second line with 'single' and \"double\" quotes && a pipe | and ; a semicolon\n\
third line with a trailing backslash \\\n\
--not-a-flag";
