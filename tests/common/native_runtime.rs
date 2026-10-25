//! Integration fixture: real Rust runtime executables across the process boundary.
//! Provider implementations are never linked into the core test binary.
pub use ahu::native::{BOUNDED, Control, DISABLED, HelperStatus, Mechanism, Profile, Request};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

static TARGET: OnceLock<tempfile::TempDir> = OnceLock::new();
extern "C" fn cleanup() {
    if let Some(target) = TARGET.get() {
        let _ = std::fs::remove_dir_all(target.path());
    }
}
fn target() -> &'static std::path::Path {
    TARGET
        .get_or_init(|| {
            let target = tempfile::Builder::new()
                .prefix("ahu-native-runtime-test-")
                .tempdir()
                .unwrap();
            // SAFETY: static callback has no captures and accesses only this process-owned fixture.
            assert_eq!(unsafe { libc::atexit(cleanup) }, 0);
            for harness in ["claude-code", "codex", "antigravity", "opencode"] {
                let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("plugins/harness-{harness}/Cargo.toml"));
                let output = Command::new(env!("CARGO"))
                    .args(["build", "--locked", "--offline", "--manifest-path"])
                    .arg(manifest)
                    .env("CARGO_TARGET_DIR", target.path())
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            target
        })
        .path()
}
fn exchange(harness: &str, calls: Vec<(&str, Value)>) -> Result<Vec<Value>, String> {
    if !["claude-code", "codex", "antigravity", "opencode"].contains(&harness) {
        return Err("unsupported".into());
    }
    let mut child = Command::new(target().join("debug").join(format!("ahu-plugin-{harness}")))
        .arg("--ahu-harness-runtime-protocol=1")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let count = calls.len();
    for (i, (operation, params)) in calls.into_iter().enumerate() {
        writeln!(
            input,
            "{}",
            json!({"protocol_version":1,"id":i+1,"operation":operation,"params":params})
        )
        .unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(rows.len(), count);
    rows.into_iter()
        .enumerate()
        .map(|(i, v)| {
            assert_eq!(v["id"], i + 1);
            assert_eq!(v["protocol_version"], 1);
            if v.get("error").is_some() {
                Err("native runtime refused request".into())
            } else {
                Ok(v["result"].clone())
            }
        })
        .collect()
}
pub fn profile(r: &Request<'_>) -> Result<Profile, String> {
    // JSON cannot represent NaN/Infinity; refuse before serialization, never turn into null.
    if r.budget_usd.is_some_and(|v| !v.is_finite()) {
        return Err("invalid input".into());
    }
    let response = exchange(
        r.harness,
        vec![(
            "admission",
            json!({"action":"native_profile","request":{
                "harness":r.harness,"harness_version":r.harness_version,"policy":r.policy,
                "session_model":r.session_model,"helper_model":r.helper_model,"helper_role":r.helper_role,
                "max_concurrent":r.max_concurrent,"max_depth":r.max_depth,"budget_usd":r.budget_usd,"assignment_writes":r.assignment_writes
            }}),
        )],
    )?;
    serde_json::from_value(response[0].clone()).map_err(|_| "invalid normalized profile".into())
}
#[derive(Default)]
pub struct Observations {
    inner: ahu::native::Observations,
    harness: Option<String>,
    events: Vec<Value>,
}
impl std::ops::Deref for Observations {
    type Target = ahu::native::Observations;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl Observations {
    pub fn observe(&mut self, harness: &str, event: &Value) {
        if harness == "other" {
            return;
        } // Unsupported parser never establishes completion.
        if let Some(prior) = &self.harness {
            assert_eq!(prior, harness);
        }
        self.harness = Some(harness.into());
        self.events.push(event.clone());
        let mut calls = vec![("decode_start", json!({"harness":harness}))];
        calls.extend(self.events.iter().map(|e| {
            (
                "decode_event",
                json!({"stream":"stdout","line":e.to_string()}),
            )
        }));
        let result = exchange(harness, calls).unwrap();
        self.inner = serde_json::from_value(result.last().unwrap()["native"].clone()).unwrap();
    }
}
