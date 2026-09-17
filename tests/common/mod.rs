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
                 record={record}\n\
                 : > \"$record\"\n\
                 for arg in \"$@\"; do printf '%s\\n' \"$arg\" >> \"$record\"; done\n\
                 exit 0\n",
                record = shell_quoted(&record.to_string_lossy())
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

/// Quote a value for `/bin/sh` so the shell reads it as one literal word.
///
/// A fixture path is chosen by `tempfile`, not by the test, and a checkout can
/// live under a directory whose name contains a quote. Interpolating such a
/// path straight into generated shell source ends the quoting early and turns
/// the rest of the path into code, so the fixture would fail — or run — for a
/// reason that has nothing to do with what the test is asserting.
pub fn shell_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// A prompt built to break anything that treats it as shell input.
pub const HOSTILE_PROMPT: &str = "Fix $(touch /tmp/ahu-pwned) and `rm -rf /` now\n\
second line with 'single' and \"double\" quotes && a pipe | and ; a semicolon\n\
third line with a trailing backslash \\\n\
--not-a-flag";

/// The environment an ahu worker session exports into its children.
///
/// When `cargo test` itself runs from inside an ahu session, these variables
/// are ambient in every test process. A test that spawns `ahu` without
/// removing them does not measure its fixture: it measures the developer's
/// live session, whose resume guard then refuses, and whose state and runtime
/// directories are the real ones.
const WORKER_ENV: &[&str] = &[
    "AHU_EXECUTION_BACKEND",
    "AHU_PARENT_TASK",
    "AHU_PARENT_ATTEMPT",
    "AHU_BROKER_TOKEN",
    "AHU_BROKER_DISPATCH",
    "AHU_FROZEN_CHILD_GRANTS",
    "AHU_EXPECTED_DIGEST",
    "AHU_EXPECTED_CHILD_IDENTITY",
    "AHU_EXPECTED_CHILD_SNAPSHOT",
    "AHU_EXPECTED_CHILD_HOOKS",
    "AHU_RUNTIME_DIR",
    "AHU_BIN",
    "AHU_STATE_DIR",
    "AHU_CMUX_BIN",
];

/// Strip the ambient worker environment from a command.
///
/// A variable removed here can still be set afterwards — `.env` overrides an
/// earlier `.env_remove` — so a test that deliberately simulates worker context
/// configures its own variables on top.
pub fn clear_worker_env(command: &mut Command) {
    for var in WORKER_ENV {
        command.env_remove(var);
    }
}

/// An `ahu` process free of the worker environment this test binary may itself
/// be running under.
///
/// Every test that runs the built `ahu` starts here, so a session running the
/// test suite cannot leak its own dispatch credentials, parent task, or state
/// locations into the process under test.
pub fn ahu() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ahu"));
    clear_worker_env(&mut command);
    command
}

/// The environment variable that tells a re-run of this test binary which case
/// it is standing in for.
pub const CHILD_CASE: &str = "AHU_TEST_CHILD_CASE";

/// Whether this process is the child running `name`.
///
/// A test whose subject is the environment or the working directory cannot set
/// those in-process: `set_var` and `remove_var` are unsafe on Unix because the
/// harness runs tests on threads, and other threads read the environment and
/// spawn processes while a mutation is in flight. A mutex over the writers does
/// not change that — the readers are in the standard library and in `Command`.
/// So the parent configures a child process instead, and the case runs there.
pub fn is_child_case(name: &str) -> bool {
    std::env::var(CHILD_CASE).as_deref() == Ok(name)
}

/// Run one of this binary's tests again in a child process, with an environment
/// and working directory the parent never mutates.
///
/// The named test must return immediately unless [`is_child_case`] says it is
/// the child, so the parent's own run of it does nothing.
pub fn run_child_case(name: &str, configure: impl FnOnce(&mut Command)) -> std::process::Output {
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_CASE, name)
        .env("RUST_BACKTRACE", "1");
    clear_worker_env(&mut command);
    configure(&mut command);
    command.output().expect("the child test runs")
}

/// Fail with the child's own output, which carries its assertion message.
///
/// Also checks the child really ran the test that was selected. A filter that
/// matches nothing exits successfully, so without this a renamed or misspelled
/// case would quietly assert nothing at all.
pub fn assert_child_passed(name: &str, output: &std::process::Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "child case {name} failed:\n{stdout}{stderr}"
    );
    assert!(
        stdout.contains("test result: ok. 1 passed"),
        "child case {name} did not run: a filter that matches nothing still exits 0.\n{stdout}{stderr}"
    );
}

/// The harness programs every adapter resolves by name.
pub const HARNESS_PROGRAMS: &[&str] = &["claude", "codex", "agy", "opencode"];

/// Run this test in a child process that has a machine of its own.
///
/// `launch::plan` resolves the harness through the process `PATH`, so a test
/// that builds a plan in-process is only hermetic if the process it runs in was
/// started with a `PATH` the test controls. Setting one from inside the test
/// would be a process-wide mutation under the harness's threads, so the parent
/// configures a child instead.
///
/// The child gets fake harness executables ahead of everything else on `PATH`,
/// a private `HOME` so no developer's `~/.claude` is read, a private state
/// directory, and an `AHU_CMUX_BIN` that is not there. `configure` runs last and
/// can change any of it — a test about ahu's behaviour with no state override
/// removes that variable.
///
/// Returns `true` in the child, where the body should run, and `false` in the
/// parent, which has by then run the child and asserted it passed.
pub fn in_child_fixture(test_name: &str, configure: impl FnOnce(&mut Command)) -> bool {
    if is_child_case(test_name) {
        return true;
    }
    let fixtures = TempDir::new().expect("fixture directory");
    let bin = fake_harnesses(fixtures.path(), HARNESS_PROGRAMS, |program| {
        fixtures.path().join(format!("{program}.argv"))
    });
    // The fixtures come first, so they answer whether or not the machine has a
    // real harness installed. The inherited entries stay, because `git` has to
    // keep resolving.
    let mut path = std::ffi::OsString::from(bin.as_os_str());
    if let Some(inherited) = std::env::var_os("PATH") {
        path.push(":");
        path.push(inherited);
    }
    let home = fixtures.path().join("home");
    let state = fixtures.path().join("state");
    for directory in [&home, &state] {
        std::fs::create_dir_all(directory).expect("fixture directory");
    }
    let output = run_child_case(test_name, |command| {
        command
            .env("PATH", &path)
            .env("HOME", &home)
            .env("AHU_STATE_DIR", &state)
            .env("AHU_CMUX_BIN", fixtures.path().join("no-such-cmux"));
        configure(command);
    });
    assert_child_passed(test_name, &output);
    false
}

/// [`in_child_fixture`] with nothing further to configure.
pub fn in_harness_fixture(test_name: &str) -> bool {
    in_child_fixture(test_name, |_| {})
}
