# Native harness services

`ahu-harness-native` is a standalone Rust implementation library for **native
runtime plugin binaries only**. It has its own workspace and lockfile, no core
path imports, and no dependency on `ahu` or the harness API crate. Core must not
link it. The runtime binary translates its validated wire request to:

```rust
pub fn call(
    harness: &str,
    operation: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, ahu_harness_native::Error>;
```

Supported harness IDs are `codex`, `claude-code`, `antigravity`, `opencode`, and
`kiro`. This library implements `probe`, `models`, `admission`, `setup`,
`native_sources`, `auth`, `budget`, `trust_prepare`, and `trust_cleanup`.
Other operations fail. Launch planning, decoding, installed package verification,
main agent supervision, tasks, and profile storage belong to other components.

`Error` implements `Display`, `Debug`, and `std::error::Error`. Display is always
`native operation refused or unavailable`; it contains no native diagnostics.
The public boolean `effects_unknown` is conservative: probe failures after
dispatch, changed snapshots, and trust transaction failures can have effects.
Never automatically replay an unknown operation. This is a library contract;
wire framing and fixed error codes are the runtime binary's responsibility.

## Inputs common to native probes

All params are JSON objects. Unknown fields and wrong types are refused,
including nested helper, skill, and import fields. Fields below are required
unless marked optional. Optional fields may be omitted or null. Inputs are
bounded to 1 MiB when serialized; complete results are bounded to 4 MiB. Paths are UTF-8 JSON strings. Existing home,
project, task and prepare-worktree directories must be absolute, canonical and
free of symlink components. Canonicalize macOS `/var` and `/tmp` aliases before
calling. File reads reject symlinks, special files, multiple hard links,
non-owned files, group/other writable files and files above 1 MiB. Admission
uses its existing bounded evidence reader and reports unsafe inputs as unknown.

`probe` and `models` take exactly:

```json
{
  "project": "/absolute/current/worktree",
  "home": "/absolute/native/home",
  "native_executable": "/absolute/selected/native-cli",
  "environment": {}
}
```

`native_executable` must name an existing absolute file. No PATH fallback is
performed. Selection, wrapper verification, executability and freezing its
identity are host responsibilities. `environment` is an explicit string-to-string
map of at most 128 entries; NULs, empty names, `=` in names and `HOME` entries are
refused. Subprocesses clear inherited environment, set HOME from `home`, set a
baseline PATH `/usr/bin:/bin:/usr/sbin:/sbin`, apply the supplied map, and force
`NO_COLOR=1`. The caller must supply any needed native wrapper PATH, credential
routing, XDG/config variables and platform settings, matching the intended native
launch. These values are transient and never included in output. Do not persist
request frames. An empty map deliberately supplies no ambient credentials.

`project` is the actual native cwd, including for probes that use native project
configuration. Subprocess stdin is closed, except the bounded Codex metadata RPC
and Antigravity status PTY. Stderr is discarded, except that PTY's combined stream.
No prompt, login, trust acknowledgement or paid model request is sent.
Recognized project configuration is fingerprinted before/after operations that
can run native code. A detected change returns unknown effects without rollback.
This detects changes; it does not prevent native code from executing or writing.

## Probe and models

`probe` invokes `--version` with an eight-second deadline and returns:

```json
{
  "version": "0.160.0",
  "reviewed_version": true,
  "compatibility": {
    "harness": "codex",
    "headless_verified_versions": ["0.154.0", "0.155.1", "0.157.1", "0.160.0"],
    "version_policy": "provider evidence text",
    "evidence": "provider evidence text",
    "unknown_integration_opt_in": false,
    "interactive_supported": true,
    "limitations": "provider limitations"
  },
  "enforcement": {
    "model": "initial_selection_only",
    "identity": "prompt_only",
    "filesystem_confinement": "not_established",
    "live_conformance": "untested"
  }
}
```

The reviewed list is extracted historical evidence, not a new live validation or
an allowlist. Decoration is removed from a semantic version; arbitrary native
version output is withheld. Special admission controls remain version-specific.

`models` returns `{"source":"native"|"suggested","models":["exact-id"],
"entitlement_checked":false}`. Codex and Claude use suggested identifiers without
executing the CLI. Antigravity/OpenCode run `models`. Kiro first checks `whoami
--format json`, then runs `chat --v3 --list-models --format json` only when the
identity reports a nonempty account type, avoiding its unauthenticated browser
flow. Each command has an eight-second deadline. Native listing failure is an
error, not a fabricated native list. Suggested IDs are not project selections.

## Admission

`admission` takes:

```json
{
  "project": "/absolute/worktree",
  "home": "/absolute/native/home",
  "native_executable": "/absolute/native-cli",
  "environment": {},
  "version": "2.29.0",
  "model": "claude-sonnet-4.5",
  "permissions": "auto",
  "mode": "headless",
  "work_mode": false,
  "overrides": [],
  "managed_claude": "/absolute/system/managed-settings.json",
  "managed_opencode": "/absolute/system/opencode",
  "managed_preferences": null,
  "system_codex": "/absolute/system/codex",
  "codex_managed_preferences": "not_applicable",
  "trust_opencode_native_state": false,
  "helpers": {
    "policy": "disabled",
    "helper_model": null,
    "helper_role": "reviewer",
    "max_concurrent": 1,
    "max_depth": 1,
    "budget_usd": 5.0,
    "assignment_writes": true
  }
}
```

`permissions` is `prompt`, `accept-edits`, or `auto`; `mode` is `interactive`,
`headless`, or `direct`. Kiro/OpenCode refuse accept-edits. OpenCode interactive
v2 refuses auto, and interactive admission checks the selected native top-level
or `mini` help declarations for required model/prompt/auto flags.

The four managed paths are optional, explicit native locations; missing relevant
managed scope blocks guarded headless admission. On macOS OpenCode requires a
managed-preferences path as well. Pass native OS locations in production, not
synthetic absent paths. `codex_managed_preferences` is a required host observation:
`not_applicable`, `absent`, `present`, or `unknown`. On macOS obtain presence via
CFPreferences for `com.openai.codex` keys `config_toml_base64` and
`requirements_toml_base64`; do not decode or send their values. Use `unknown` if
unavailable. The host supplies this observation because fixture inspection must
not silently query the executing machine's managed preferences.

`overrides` contains presence-only environment names (at most 128 uppercase
ASCII/digit/underscore names). Supplied environment keys are also considered.
Native configuration relocation and indirection remain unknown and block guarded
headless admission. Never omit effective native override names to pass admission.
`trust_opencode_native_state` must come from the project's explicit policy, not a
plugin-generated authorization. It permits only the existing narrowly classified
native account/database state and discloses the uncertainty.

`version` must come from the selected executable's frozen probe. Results are:

- `allowed`: boolean; core must reject `false` without a fallback.
- `decision`: `admitted`, `refused`, or `work_mode`.
- `guarded_checks`: `inspected` or `intentionally_skipped`.
- `conformance`: always `untested`.
- `evidence`: the native evidence object below.
- `helper_profile`: required arguments, environment additions/removals, controls
  and gaps for the requested helper policy.
- `invocation_profile`: null or `{id,args}` for version-specific guarded controls.

The evidence object has `harness`, `native_agent` (nullable), `profile` (the
compatibility object), `cli_version` (nullable), `version_checked`, `components`,
`gaps`, `conformance`, `headless:{allowed,reasons,evidence_digest}`, and
`next_action`. Each component has `name`, `registration` (`installed`, `missing`,
`unknown`), `activation` (`disabled`, `unknown`, `not_applicable`), `isolation`
(`absent`, `verified_disable`, `unknown`), `conformance` (`untested`,
`source_inspected`), `detail` and `evidence:[{path,scope,method,digest}]`.
Paths and digests are private runtime evidence, not public context-lock material.
Bodies, commands, credentials and native metadata responses are not returned.

The helper profile has `policy`, `harness`, `args`, `env` (name/value pairs),
`env_remove`, `helper_tools`, `helper_model`, `max_concurrent`, `max_depth`,
`enforced:[{id,mechanism,detail}]`, and `gaps`. `mechanism` is `flag`, `env`, or
`structural`. `policy` is `disabled` or `bounded`. Bounded remains limited to
Claude 2.1.270, a read-only owning assignment, depth one, positive concurrency,
a valid role/model and optional positive USD budget. Unsupported bounded requests
refuse; no policy is silently substituted. For Codex disabled helpers, returned
args include the adapter's required `-c agents.enabled=false` control.

The runtime must apply **both** returned profiles to the same native launch,
without dropping or overriding controls. Claude guarded profiles preserve managed
settings while disabling non-managed hooks. Codex 0.157.1/0.160.0 performs fresh
`hooks/list` and `configRequirements/read` RPC with its exact strict profile;
hooks must be empty and mandatory requirements explicitly null. Kiro guarded
headless configuration supports the inspected 2.28.0/2.29.0 subset. Unknown
hooks, extensions, executable modules, cloud sources and unreviewed formats
remain refusals. Work mode retains evidence and helper checks but intentionally
skips guarded admission; `allowed:true` then is not conformance. Interactive and
direct mode disclose unresolved configuration without imposing headless policy.

## Setup proposals

`setup` takes `project`, `home`, `ahu_executable` (absolute existing file), and
`skills:[{name,text}]`. It never invokes a native executable or writes a file.
Skill names are bounded ASCII letters/digits/hyphen/underscore; duplicate names
and conflicting installed text refuse the whole proposal. Supply the actual
packaged skill text; the library does not infer skill payloads or install agents.

It returns:

```json
{
  "changes": [{
    "path": "/absolute/project/.mcp.json",
    "expected_before_sha256": null,
    "text": "proposed UTF-8 file contents"
  }],
  "publication": "host_transaction_required"
}
```

A null digest means **must be absent**, otherwise it is the lowercase SHA-256 of
the exact prior bytes. Every change is a complete file proposal, not a patch.
Unchanged files are omitted. Maximum generated file size is 1 MiB and aggregate
serialized changes are at most 4 MiB. The host must validate path grants, compare
all before states, and apply the set through its conflict-checked transaction.
Native writers do not share this transaction; the host must account for races.
Never treat these returned paths as general filesystem authority.

| Provider | MCP target | Format |
| --- | --- | --- |
| Codex | project `.codex/config.toml` | `mcp_servers.ahu`, TOML preserving comments |
| Claude | project `.mcp.json` | `mcpServers.ahu` |
| Antigravity | home `.gemini/config/mcp_config.json` | `mcpServers.ahu`, explicit native-global proposal |
| OpenCode | project `opencode.json` | local `mcp.ahu` command array |
| Kiro | project `.kiro/settings/mcp.json` | `mcpServers.ahu` |

Existing unrelated fields are retained; malformed/conflicting entries refuse.
OpenCode JSONC refuses instead of creating a competing JSON file. Antigravity
accepts empty configuration and preserves explicit enabled entries. Kiro inserts
literal native `${NAME}` forwarding for `AHU_TASK_ID`, `AHU_TASK_DIR`,
`AHU_EVAL_OTEL_ENDPOINT`, `AHU_MCP_RESOURCE_ATTRIBUTES`, and
`OTEL_RESOURCE_ATTRIBUTES`, refusing conflicting values. Codex adds these names
to `env_vars`, and proposes `approval_mode="approve"` for missing
`ahu_agents_list`, `ahu_tasks_list`, `ahu_task_get`, `ahu_request_approval` tool
entries. Existing explicit per-tool policy remains unchanged. These are
reviewable setup proposals, not hidden runtime approvals.

All providers receive requested skills under `.agents/skills/<name>/SKILL.md`;
Claude and Kiro also receive their native `.claude/skills` or `.kiro/skills` copy.
Setup validates ownership, writability and symlink safety from the selected
project/home anchor through each target.

## Passive sources and imports

`native_sources` takes `project`, `home`, and optional
`import:{path,format}`. It runs no code, activates no settings and does not probe
authentication. It returns `sources:[{kind,path,present,loaded:"unknown"}]`,
`imported` (null or the import below), `activation:"none"`, and `coverage`.
It inventories recognized context/config/agent/skill roots in the project and
native home; it does not prove loading or enumerate synchronized remote context.
Source roots are provider-specific (`.claude`, `.codex`, `.agents`, `.opencode`,
`.kiro`) and include native MCP/config files. Inventory refuses linked paths.

Imports require an explicit absolute regular path inside `project`. Supported
pairs: Claude `claude-agent`, Codex `codex-agent`, Antigravity
`antigravity-agent`, OpenCode `opencode-agent`, Kiro `kiro-json` and
`kiro-markdown`. Markdown supports YAML frontmatter; Codex preserves the complete
source text verbatim, matching existing core semantics. Kiro JSON accepts inline
`prompt`, refusing `file://` prompt indirection. The result contains `path`,
`format`, `instructions`, `model` (nullable), `declared_setting_fields`, `sha256`,
and `settings_applied:false`. Non-string models, malformed frontmatter, missing
or empty instructions and path escapes refuse. Imported settings are never
activated. Core must compare nonempty, non-`inherit` models to the frozen model,
compute its instruction digest, and register identities through its normal flow.

## Task trust

Both trust operations take exactly:

```json
{
  "task_dir": "/absolute/verified/task-state",
  "worktree": "/absolute/verified/task-worktree",
  "home": "/absolute/original/native/home",
  "permissions": "auto",
  "mode": "interactive",
  "overrides": []
}
```

Before dispatch core must verify the frozen task identity, native runtime package,
repository identity, exact canonical task-owned Git worktree, state-directory
ownership and authorization. A path shaped like a worktree is not sufficient.
This library cannot establish task ownership without importing core task types.
Never expose these calls as arbitrary checkout trust setters. Trust is supported
only for interactive auto Codex, Claude and Antigravity; other modes/providers
refuse. The `overrides` presence list must reflect the original invocation;
`CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `GEMINI_CLI_HOME` and `AGY_CONFIG_DIR` refuse for
their respective providers. Relocated native stores are unsupported.

Preparation preserves preexisting trust, writes a bounded `native-trust.json`
intent receipt **before** changing native trust, and edits only the exact worktree
entry. Cleanup reconciles that receipt, restores prior false/untrusted values,
preserves native history/unrelated entries and removes only task-owned additions.
It handles interruption before or after the native write, native revocation and
deleted native stores. Cleanup accepts a removed worktree, but requires the
original home and task directory. It never runs a harness subprocess or answers
a native approval prompt. Return value is `{status:"complete",receipt_path}`;
the path may be absent when trust preexisted or cleanup completed.

Receipt schema/filename and native formats are retained from the existing trust
implementation. Files are owner-only atomic durable replacements with compare
before replacement, link/ownership/mode/size checks, a stable per-home flock
(deadline ten seconds), and directory fsync. Existing receipts with mismatched
home/worktree/harness refuse. Failed transactions preserve intent for inspection
and cleanup. A lost response or wrapper timeout must be treated as unknown; keep
the receipt and explicitly reconcile with the same frozen runtime. The lock
coordinates cooperating plugin writers only. Native applications and same-user
hostile processes are not atomically isolated.

## Account and capacity

`auth` and `budget` take the probe fields plus optional `model` and `ollama_host`.
OpenCode auth uses `model` or the explicit requested identity described below.
Its Ollama cloud route uses a validated loopback host; other OpenCode providers
return unsupported. No alternate account is selected.

Successful auth returns `{status:"available",binding_harness,fingerprint,
effective_provider_address_verified,identity_kind}`. Unsupported OpenCode auth returns
`{status:"unsupported",binding_harness:null,fingerprint:null}`. The fingerprint
preserves the existing SHA-256 over serde_json's sorted-key object containing
`harness`, `identity_kind`, `principal`, `organization`, and `workspace` including
nulls. Principals, organizations, workspace IDs and raw account responses are
never returned. Core keeps binding/profile storage and comparisons.

- Claude runs `auth status --json` (eight seconds), requiring logged-in identity.
- Codex reads account metadata via app-server (ten seconds), accepting ChatGPT
  identity and preserving workspace routing in the fingerprint. API-key identity
  is explicitly unavailable.
- Kiro runs `whoami --format json` (eight seconds), accepting the inspected
  `SocialGitHub` identity; any supplied `KIRO_API_KEY` refuses before probing.
- Antigravity observes its startup PTY without sending input (twelve seconds,
  64 KiB, 2.5-second identity settling interval). Folder-trust prompts and API-key
  banners refuse. This is a bounded startup observation, not a native standalone
  status API or proof of complete late identity fields.
- OpenCode routes `ollama/*:cloud*` or `ollama/*-cloud` case-insensitively to the
  explicit loopback Ollama `/api/me` POST. Timeout is four seconds, redirects and
  proxies disabled, response at most 64 KiB. It preserves `ollama-account`
  semantics and the stable account ID. It cannot verify OpenCode's effective
  provider address; `effective_provider_address_verified` is false for this route.

Codex `budget` reads account and rate limits in one native app-server session and
returns `status` (`available` or `unknown`), `binding_harness:"codex"`,
`fingerprint`, `signal_type` (`rate_windows` or `unknown`),
`source:"codex_app_server"`, `scope:"shared_account"`, `observed_at_unix`, and
`windows`. Each window has `name`, `used_percent`, `remaining_percent`,
`window_duration_minutes`, and nullable `resets_at_unix`. Any malformed, expired,
or incomplete present window makes the entire capacity observation unknown with
empty windows. Core must compare the returned fingerprint against the active
profile before presenting capacity as identity-bound. No reservation, token
budget or cost estimate is implied. All other providers return
`{status:"unsupported",windows:[],signal_type:"unknown"}` without account probes.
No operation switches accounts or writes credentials intentionally.

## Integration and validation limits

The consumer is the installed runtime binary, not core. A machine service is
needed for native formats, bounded RPC, trust reconciliation and setup proposals;
a text skill cannot supply this contract. No shared API types or wire framing
are duplicated here. The wrapper should allow at least 35 seconds for the
30-second Codex admission RPC plus filesystem inspection, and classify wrapper
loss/timeouts as unknown effects. Every native subprocess has an owned process
group and bounded in-memory output; stdout readers do not wait indefinitely for
detached descendants' EOF. Detached or provider-managed work is not proven
stopped. Process spawn itself and local filesystem syscalls are not interruptible
deadlines; the outer installed-runtime deadline remains necessary.

Project snapshots cover recognized top-level native/context trees, to depth 16,
8,192 entries, 1 MiB per file and 32 MiB total. Overflow refuses. The host still
owns generic repository snapshots/context locks, ancestor-discovered inputs,
freeze/compare of package/executable/environment identity and launch-time race
handling. There is no claim of prevention, whole-home snapshotting or sandboxing.
Native RPC and source-format evidence is bounded; actual native loading, account
availability and provider execution require separate live integration validation.

The implementation extracts focused admission, helper profile, trust, Codex
metadata and identity logic; it does not copy task/process supervision or core
profile storage. Synthetic tests use temporary homes and fake CLIs, never actual
user homes, external providers or paid models. No test creates a Git worktree.
Run focused checks without core suites:

```sh
cargo fmt --manifest-path plugins/harness-native/Cargo.toml --check
CARGO_TARGET_DIR=/tmp/ahu-full-plugin-native-target cargo clippy --manifest-path plugins/harness-native/Cargo.toml --all-targets --locked -- -D warnings
CARGO_TARGET_DIR=/tmp/ahu-full-plugin-native-target cargo test --manifest-path plugins/harness-native/Cargo.toml --locked
```

These checks establish the standalone implementation boundary, not installed
runtime integration or completion of whole-core harness extraction. Dependency
release/advisory review and live native conformance remain coordinator gates.

## Provider-selected settings and core facades

New consumers use provider-selected settings instead of constructing native
location fields. The explicit-location admission shape above remains supported
for synthetic fixtures. Do not mix the shapes. The new shape is:

```json
{
  "project": "/absolute/worktree",
  "home": "/absolute/native/home",
  "native_executable": "/absolute/selected/native-cli",
  "environment": {"PATH": "/usr/bin:/bin"},
  "environment_names": ["PATH"],
  "version": "1.2.3",
  "model": "exact-model",
  "permissions": "auto",
  "mode": "interactive",
  "work_mode": false,
  "settings": {
    "inspection_only": false,
    "trust_native_account_state": false
  }
}
```

Settings reject unknown fields. Optional `settings.helpers` uses the exact
helper object documented above; absent means disabled delegation. The plugin
selects managed/system paths and macOS preferences. `environment_names` is a
presence-only list of at most 512 names, each at most 256 bytes, without NUL or
an equals sign. Values are not included. Actual subprocess variables still come
exclusively from `environment`.

With `inspection_only:true`, version/model/permissions/mode/work_mode may be
omitted. The result always has `allowed:false`, `decision:"inspection"`,
`guarded_checks:"inventory_only"` and unverified conformance. Inventory alone
cannot admit a launch. Without a model, guarded admission requires `version`
and checks native configuration/isolation; full launch admission additionally
requires exact model, permissions and mode. Work mode with full inputs reports
`guarded_checks:"intentionally_skipped"`, never fabricated conformance.

Auth and budget additionally accept optional `environment_names` and
`requested_identity`. Recognized native account/config override names absent
from the explicit environment cause refusal before probing, preventing an
environment-cleared wrapper from binding a different default account.
Requested identity normally equals the runtime harness ID. OpenCode also accepts
`ollama`, for explicit account binding without a model; the installed descriptor
must declare that alias so generic core can resolve it. Cloud model routing
remains provider-owned. Explicit `ollama_host` precedes environment OLLAMA_HOST;
absent both, the native default is http://127.0.0.1:11434. Masked OLLAMA_HOST still
refuses. Successful identity results also include `identity_kind`, preserving
profile metadata while withholding the principal.

Native sources additionally accepts optional
`settings:{"view":VIEW,"environment_names":[...]}`. Exact views:

- Absent or `inventory`: the source/import shape documented above.
- `hooks`: a HookInventory object with `hooks`, `settings`, `mcp_servers`,
  `declared_plugins`, `unreadable`, `checked`, and nullable `unscanned_harness`.
  Hooks contain event/matcher/kind/scope/source/command_digest, never raw commands.
  Settings facts include plugin-computed `approvals_widened`. MCP commands are
  digest labels. Unknown surfaces and overrides remain explicitly unscanned.
  Reads are bounded to 1 MiB; special files refuse without blocking.
- `private_fingerprints`: `{"fingerprints":{"native:HARNESS:INDEX":"SHA256"}}`,
  only digests of existing native configuration. Overrides refuse rather than
  fingerprinting a different location. Keys are opaque private-lock entries.

Unknown views/fields refuse. Inventory never activates native agents or hooks.

The generic core setup facade is
`plan(repo,harness,ahu_executable,skills,allowed_roots) -> Vec<Plan>`, with
`Plan { path, expected_before_sha256, text }`. `parse_proposals` checks size,
unique paths, caller-authorized scope and before-state; `Plan::check` rechecks
before publication. Publication and rollback belong to the setup transaction.
The facade does not expose the old single-path formatter or mutate files.

Core trust dispatch first writes `native-runtime-trust-binding.json`, pinning
task ID/directory, repository, worktree, identity digest, home and runtime binding
digest. Prepare verifies the launch's frozen planner binding. Cleanup requires
the same binding and home; it never resolves a replacement provider silently.
A legacy `native-trust.json` without that marker requires explicit legacy
recovery. Providers lacking preparation record a no-op marker; providers
advertising preparation must also support cleanup. The provider's separate
write-ahead receipt governs native changes and reconciliation. Ineligible
modes/permissions/providers return
`{"status":"not_applicable","receipt_path":null}` only when no native receipt
exists; changing eligibility cannot bypass an existing receipt.

Core retains profile storage, fingerprint comparisons, ownership, rendering and
bounded result validation. `Status.profile` is owned `ProfileEvidence`, not a
static catalog reference. External callers must migrate provider-specific
private-lock keys to `native_mcp::fingerprints(repo,harness)` and classify
`native:` entries as private. Native parser tests belong here; core tests use
invented providers and generic observations.

## Exact extraction operations

Native source calls also accept these mutually exclusive, strict action forms:

- `{"action":"parse","format":string,"text":string,"path":string}` returns
  exactly `{instructions:string,native_model:string|null,native_settings:object}`.
  Settings values are strings; structured declared values are JSON strings and
  never executed. Formats are `codex-agent`, `claude-agent`, `opencode-agent`,
  `antigravity-agent`, `kiro-json`, and `kiro-markdown`, only for their provider.
  Codex TOML is delivered verbatim with null native_model and empty settings;
  its TOML model key never overrides the manifest. Markdown frontmatter models
  are returned for core's fixed-model conflict check. Kiro file:// prompt imports
  refuse. Parse reads no paths and accepts at most 1 MiB of text.
- `{"action":"discover","project_dir":absolute_canonical_path}` returns an
  array of exactly `{name,path,format,native_model,blockers,preserved_fields,
  description}`. Paths are project-relative. Discovery reads at most 1,024
  directory entries and returns at most 256 candidates, bounded to 1 MiB per
  definition. Symlinks refuse; subdirectories are not recursively traversed,
  except the recognized Antigravity `<name>/agent.md` layout. Invalid definitions
  carry a fixed blocker. No registration or write occurs.

Admission also accepts `{"action":"native_profile","request":REQUEST}` with
optional canonical `project`. REQUEST contains exactly `harness`,
`harness_version`, `policy`, `session_model`, nullable `helper_model`,
`helper_role`, `max_concurrent`, `max_depth`, nullable `budget_usd`, and
`assignment_writes`. The result is the existing helper Profile shape (policy,
harness, args, env, env_remove, helper_tools, helper_model, max_concurrent,
max_depth, enforced, gaps). Provider mismatch refuses. Disabled/bounded helper
constraints remain enforced. Codex's agents.enabled=false and version-specific
invocation-isolation flags are included in the frozen profile. This pure
operation does not establish native configuration admission or live conformance.
Guarded admission requires an exact live native model-list match
for OpenCode and Kiro before accepting the requested invocation.

`recover` accepts exactly `harness`, `native_version`, `native_executable`,
nullable `expected_executable_digest`, `worktree`, `session`, and
`attempt_started_ms`, plus optional canonical `home` and string-map
`environment`. It returns exactly `{"recovered":bool}`. Only OpenCode 2.0.32 is
eligible; unsupported versions/providers or absent frozen executable digest
return false without execution. A digest mismatch refuses. It invokes only
`session export --sanitize --standalone SESSION`, with no stdin or model replay,
within one 30-second deadline and 1 MiB output bound. Only non-JSON exports retry;
a valid but insufficient export returns false. The event crate's pure validator
checks session identity, attempt timestamps, final completion and tool outcomes.
Native output never crosses into core. Executable bytes and recognized worktree
inputs are checked again after retrieval. Outer timeout or lost response remains
unknown effects. Optional explicit home/environment should match the launch;
without home, no HOME override is passed and native default-home selection
applies. The subprocess environment is cleared and OTel is disabled for this
inspection, without altering the main task's telemetry configuration.

Native trust cleanup reconciles an existing owned receipt even when current
permissions/mode no longer qualify for preparation. Ineligible preparation with
no receipt is a no-op, including relocated configuration overrides. Core cleanup
uses the frozen home without requiring the native CLI to remain installed.

Installed descriptors declare `private_context_paths`, a bounded list of plain
relative files/subtrees already covered by context_paths. Core filters these
from shared lock entries and keeps their acceptance in private owner state.
Native fingerprint inspection likewise does not resolve or execute the native
CLI. The fixed provider filename list lives only in this implementation crate.

`admission` also accepts `action:"execution_capabilities"`, `harness`,
`native_version`, `permissions` (`prompt|accept-edits|auto`), `work_mode`, and
optional `native_helpers` (`disabled|bounded`, default disabled). It returns
exactly `{structured_output:bool,broker_transport:bool}`. Guarded Codex prompt
permissions deny both write-dependent capabilities; guarded bounded helpers deny
structured output. Work mode skips those guarded limits. These pure permission
checks do not certify native configuration, supported versions, or live behavior;
normal admission remains required. Unknown fields or mismatched harness refuse.
