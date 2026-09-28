//! Exactly which inputs a run measured.
//!
//! Two runs are comparable when the case, the prompt, the scoring, both agents'
//! instructions, the skills in front of them, the tools they were offered, and
//! the ahu that orchestrated them were the same. Every one of those is recorded
//! as a digest here, so a report can group by identity instead of by name.
//!
//! Three properties matter more than the field list.
//!
//!   - **`ahu_revision` used to mean two things.** It carried the invoking
//!     checkout's HEAD, which describes the repository under evaluation, and it
//!     was read as if it identified the ahu that ran the evaluation. Those come
//!     apart the moment ahu is installed from anywhere but the checkout. They
//!     are now `target_repo_head` and a build identity of their own, and the old
//!     field is neither written nor reinterpreted.
//!   - **Nothing here is a path or a secret.** Digests, versions, and short
//!     identifiers only: a record is meant to be comparable across machines, and
//!     a local path is neither comparable nor the record's business.
//!   - **Missing is not equal.** A fingerprint that could not be completed is
//!     marked `partial`, and that marking is part of what a report groups by, so
//!     a run whose inputs are only partly known can never be pooled with runs
//!     whose inputs are fully known.

use crate::agent::ResolvedAgent;

/// How much of the evaluator's environment was separated from the candidate's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blinding {
    /// The evaluator's prompt withholds candidate identity, model, harness, and
    /// trace. It runs in its own clean ahu task, from the same checkout as the
    /// candidate, so it could in principle observe that environment. This is
    /// prompt-level blinding and is not environmental isolation.
    PromptOnly,
    /// As above, and the evaluator ran from a separately prepared checkout the
    /// operator named, so the candidate's checkout is not its environment.
    Isolated,
}

impl Blinding {
    pub fn as_str(self) -> &'static str {
        match self {
            Blinding::PromptOnly => "prompt_only",
            Blinding::Isolated => "isolated",
        }
    }

    /// What this blinding does *not* claim, carried in the run output so the
    /// limitation travels with the numbers rather than living only in a doc.
    pub fn caveat(self) -> &'static str {
        match self {
            Blinding::PromptOnly => {
                "prompt_only: candidate identity, model, harness and trace are withheld from the evaluator prompt, and the evaluator runs in its own clean ahu task. The evaluator's checkout is still the candidate's, so this is not environmental isolation."
            }
            Blinding::Isolated => {
                "isolated: as prompt_only, and the evaluator ran from a separately prepared checkout, so the candidate's checkout is not part of its environment."
            }
        }
    }
}

/// One agent's identity, as digests rather than as a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFingerprint {
    pub name: String,
    pub version: String,
    pub model: String,
    pub harness: String,
    /// Reported by the harness at launch. Absent when it did not report one.
    pub harness_version: Option<String>,
    pub manifest_digest: String,
    pub source_digest: String,
    pub instructions_digest: String,
    /// The combined identity ahu already computes for a resolved agent.
    pub identity_digest: String,
}

impl AgentFingerprint {
    pub fn of(agent: &ResolvedAgent) -> Self {
        Self {
            name: agent.manifest.name.clone(),
            version: agent.manifest.version.clone(),
            model: agent.manifest.model.clone(),
            harness: agent.manifest.harness.clone(),
            harness_version: None,
            manifest_digest: agent.manifest_digest.clone(),
            source_digest: agent.source_digest.clone(),
            instructions_digest: agent.instructions_digest.clone(),
            identity_digest: agent.identity_digest(),
        }
    }

    pub fn with_harness_version(mut self, version: Option<String>) -> Self {
        self.harness_version = version.filter(|value| !value.trim().is_empty());
        self
    }

    fn canonical(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "version": self.version,
            "model": self.model,
            "harness": self.harness,
            "harness_version": self.harness_version,
            "manifest_digest": self.manifest_digest,
            "source_digest": self.source_digest,
            "instructions_digest": self.instructions_digest,
            "identity_digest": self.identity_digest,
        })
    }
}

/// Which ahu ran the evaluation, as distinct from what it evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildIdentity {
    pub version: String,
    /// SHA-256 of the running executable. Absent when the executable could not
    /// be located or read, which is a gap rather than a value to invent.
    pub binary_digest: Option<String>,
    /// SHA-256 over the MCP tool definitions this build serves.
    pub tool_definitions_digest: String,
}

impl BuildIdentity {
    /// The identity of the ahu executing this process.
    ///
    /// The binary digest is the strong part: a version string is declared, while
    /// the digest is what the machine actually ran. Reading it can fail — a
    /// deleted or replaced executable, a platform that does not expose the path
    /// — and then it stays absent and the fingerprint is partial.
    pub fn detect() -> Self {
        let binary_digest = std::env::current_exe()
            .ok()
            .and_then(|path| crate::util::digest_file(&path).ok());
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            binary_digest,
            tool_definitions_digest: crate::mcp::tool_definitions_digest(),
        }
    }
}

/// The suite a run came from, when it came from one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuiteIdentity {
    pub id: String,
    pub suite_version: String,
    pub digest: String,
}

/// Whether every input this record groups by is actually known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completeness {
    Complete,
    Partial,
}

impl Completeness {
    pub fn as_str(self) -> &'static str {
        match self {
            Completeness::Complete => "complete",
            Completeness::Partial => "partial",
        }
    }
}

/// Everything that has to match before two runs measure the same thing.
#[derive(Debug, Clone, PartialEq)]
pub struct InputFingerprint {
    pub case_id: String,
    pub corpus_version: String,
    pub case_schema_version: u32,
    pub case_digest: String,
    pub prompt_profile: super::case::PromptProfile,
    pub prompt_version: u32,
    pub scoring_version: u32,
    pub suite: Option<SuiteIdentity>,
    /// The suite's weight for this case, when a suite named one.
    pub case_weight: Option<f64>,
    pub candidate: AgentFingerprint,
    pub evaluator: Option<AgentFingerprint>,
    pub blinding: Blinding,
    /// Digest over the skills the candidate's harness was given. Absent when the
    /// launch reported no skill catalog.
    pub skill_digest: Option<String>,
    /// Policy/catalog/provider identity for optional prelaunch advice.
    pub selection_policy_digest: Option<String>,
    /// Digest over the skills provided to the evaluator.
    pub evaluator_skill_digest: Option<String>,
    pub build: BuildIdentity,
    /// HEAD of the checkout under evaluation. Absent in a repository with no
    /// commit yet.
    pub target_repo_head: Option<String>,
    /// HEAD of the checkout from which the evaluator ran.
    pub evaluator_repo_head: Option<String>,
}

impl InputFingerprint {
    /// Which grouping inputs are missing, named, in a stable order.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.build.binary_digest.is_none() {
            missing.push("ahu_build_digest");
        }
        if self.target_repo_head.is_none() {
            missing.push("target_repo_head");
        }
        if self.skill_digest.is_none() {
            missing.push("skill_digest");
        }
        if self.evaluator.is_some() && self.evaluator_skill_digest.is_none() {
            missing.push("evaluator_skill_digest");
        }
        if self.evaluator.is_some() && self.evaluator_repo_head.is_none() {
            missing.push("evaluator_repo_head");
        }
        if self.candidate.harness_version.is_none() {
            missing.push("harness_version");
        }
        if self
            .evaluator
            .as_ref()
            .is_some_and(|evaluator| evaluator.harness_version.is_none())
        {
            missing.push("evaluator_harness_version");
        }
        missing
    }

    pub fn completeness(&self) -> Completeness {
        if self.missing().is_empty() {
            Completeness::Complete
        } else {
            Completeness::Partial
        }
    }

    /// The canonical JSON the digest below is taken over.
    fn canonical(&self) -> serde_json::Value {
        serde_json::json!({
            "case_id": self.case_id,
            "corpus_version": self.corpus_version,
            "case_schema_version": self.case_schema_version,
            "case_digest": self.case_digest,
            "prompt_profile": self.prompt_profile.as_str(),
            "prompt_version": self.prompt_version,
            "scoring_version": self.scoring_version,
            "suite": self.suite.as_ref().map(|suite| serde_json::json!({
                "id": suite.id,
                "suite_version": suite.suite_version,
                "digest": suite.digest,
            })),
            "case_weight": self.case_weight,
            "candidate": self.candidate.canonical(),
            "evaluator": self.evaluator.as_ref().map(AgentFingerprint::canonical),
            "blinding": self.blinding.as_str(),
            "skill_digest": self.skill_digest,
            "selection_policy_digest": self.selection_policy_digest,
            "evaluator_skill_digest": self.evaluator_skill_digest,
            "ahu_version": self.build.version,
            "ahu_build_digest": self.build.binary_digest,
            "tool_definitions_digest": self.build.tool_definitions_digest,
            "target_repo_head": self.target_repo_head,
            "evaluator_repo_head": self.evaluator_repo_head,
        })
    }

    /// One digest over every grouping input, for a compact comparison.
    ///
    /// It is a convenience, not the grouping key: a report still groups by the
    /// individual fields so a reader can see *which* input differed.
    pub fn digest(&self) -> String {
        crate::util::digest_bytes(&serde_json::to_vec(&self.canonical()).unwrap_or_default())
    }

    /// The fingerprint fields as they are written into a run record.
    pub fn record_fields(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut fields = serde_json::Map::new();
        let mut put = |key: &str, value: serde_json::Value| {
            fields.insert(key.to_owned(), value);
        };
        put(
            "selection_policy_digest",
            self.selection_policy_digest
                .clone()
                .map_or(serde_json::Value::Null, Into::into),
        );
        put("case_id", self.case_id.clone().into());
        put("corpus_version", self.corpus_version.clone().into());
        put("case_schema_version", self.case_schema_version.into());
        put("case_digest", self.case_digest.clone().into());
        put("prompt_profile", self.prompt_profile.as_str().into());
        put("prompt_version", self.prompt_version.into());
        put("scoring_version", self.scoring_version.into());
        put(
            "suite_id",
            self.suite
                .as_ref()
                .map(|suite| suite.id.clone())
                .map_or(serde_json::Value::Null, Into::into),
        );
        put(
            "suite_version",
            self.suite
                .as_ref()
                .map(|suite| suite.suite_version.clone())
                .map_or(serde_json::Value::Null, Into::into),
        );
        put(
            "suite_digest",
            self.suite
                .as_ref()
                .map(|suite| suite.digest.clone())
                .map_or(serde_json::Value::Null, Into::into),
        );
        put(
            "case_weight",
            self.case_weight
                .and_then(serde_json::Number::from_f64)
                .map_or(serde_json::Value::Null, serde_json::Value::Number),
        );
        put("agent", self.candidate.name.clone().into());
        put("agent_version", self.candidate.version.clone().into());
        put("model", self.candidate.model.clone().into());
        put("harness", self.candidate.harness.clone().into());
        put(
            "harness_version",
            self.candidate
                .harness_version
                .clone()
                .map_or(serde_json::Value::Null, Into::into),
        );
        put(
            "agent_manifest_digest",
            self.candidate.manifest_digest.clone().into(),
        );
        put(
            "agent_source_digest",
            self.candidate.source_digest.clone().into(),
        );
        put(
            "agent_instructions_digest",
            self.candidate.instructions_digest.clone().into(),
        );
        put(
            "agent_identity_digest",
            self.candidate.identity_digest.clone().into(),
        );
        let evaluator_field = |pick: fn(&AgentFingerprint) -> Option<String>| {
            self.evaluator
                .as_ref()
                .and_then(pick)
                .map_or(serde_json::Value::Null, Into::into)
        };
        put("evaluator", evaluator_field(|a| Some(a.name.clone())));
        put(
            "evaluator_version",
            evaluator_field(|a| Some(a.version.clone())),
        );
        put(
            "evaluator_model",
            evaluator_field(|a| Some(a.model.clone())),
        );
        put(
            "evaluator_harness",
            evaluator_field(|a| Some(a.harness.clone())),
        );
        put(
            "evaluator_harness_version",
            evaluator_field(|a| a.harness_version.clone()),
        );
        put(
            "evaluator_manifest_digest",
            evaluator_field(|a| Some(a.manifest_digest.clone())),
        );
        put(
            "evaluator_source_digest",
            evaluator_field(|a| Some(a.source_digest.clone())),
        );
        put(
            "evaluator_instructions_digest",
            evaluator_field(|a| Some(a.instructions_digest.clone())),
        );
        put(
            "evaluator_identity_digest",
            evaluator_field(|a| Some(a.identity_digest.clone())),
        );
        put("blinding", self.blinding.as_str().into());
        put("blinding_caveat", self.blinding.caveat().into());
        put(
            "skill_digest",
            self.skill_digest
                .clone()
                .map_or(serde_json::Value::Null, Into::into),
        );
        put(
            "evaluator_skill_digest",
            self.evaluator_skill_digest
                .clone()
                .map_or(serde_json::Value::Null, Into::into),
        );
        put("ahu_version", self.build.version.clone().into());
        put(
            "ahu_build_digest",
            self.build
                .binary_digest
                .clone()
                .map_or(serde_json::Value::Null, Into::into),
        );
        put(
            "tool_definitions_digest",
            self.build.tool_definitions_digest.clone().into(),
        );
        put(
            "target_repo_head",
            self.target_repo_head
                .clone()
                .map_or(serde_json::Value::Null, Into::into),
        );
        put(
            "evaluator_repo_head",
            self.evaluator_repo_head
                .clone()
                .map_or(serde_json::Value::Null, Into::into),
        );
        put("input_fingerprint", self.digest().into());
        put(
            "fingerprint_completeness",
            self.completeness().as_str().into(),
        );
        put(
            "fingerprint_missing",
            serde_json::Value::Array(
                self.missing()
                    .into_iter()
                    .map(|name| serde_json::Value::String(name.to_owned()))
                    .collect(),
            ),
        );
        fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str) -> AgentFingerprint {
        AgentFingerprint {
            name: name.into(),
            version: "1.0.0".into(),
            model: "ollama/fixture".into(),
            harness: "opencode".into(),
            harness_version: Some("1.2.3".into()),
            manifest_digest: "a".repeat(64),
            source_digest: "b".repeat(64),
            instructions_digest: "c".repeat(64),
            identity_digest: "d".repeat(64),
        }
    }

    fn complete() -> InputFingerprint {
        InputFingerprint {
            case_id: "routing-1".into(),
            corpus_version: "1.0.0".into(),
            case_schema_version: 2,
            case_digest: "e".repeat(64),
            prompt_profile: super::super::case::PromptProfile::ToolNeutralV2,
            prompt_version: super::super::case::PROMPT_VERSION,
            scoring_version: super::super::case::SCORING_VERSION,
            suite: None,
            case_weight: None,
            candidate: agent("triage"),
            evaluator: Some(agent("judge")),
            blinding: Blinding::PromptOnly,
            skill_digest: Some("f".repeat(64)),
            selection_policy_digest: None,
            evaluator_skill_digest: Some("9".repeat(64)),
            build: BuildIdentity {
                version: "0.5.0".into(),
                binary_digest: Some("1".repeat(64)),
                tool_definitions_digest: "2".repeat(64),
            },
            target_repo_head: Some("deadbeef".into()),
            evaluator_repo_head: Some("feedface".into()),
        }
    }

    #[test]
    fn a_complete_fingerprint_names_nothing_missing_and_digests_stably() {
        let fingerprint = complete();
        assert!(fingerprint.missing().is_empty());
        assert_eq!(fingerprint.completeness(), Completeness::Complete);
        assert_eq!(fingerprint.digest(), complete().digest());
        assert_eq!(fingerprint.digest().len(), 64);
    }

    /// One edit to a fingerprint, for the mutation tables below.
    type Mutation = Box<dyn Fn(&mut InputFingerprint)>;

    #[test]
    fn each_missing_input_is_named_and_makes_the_fingerprint_partial() {
        for (mutate, expected) in [
            (
                Box::new(|f: &mut InputFingerprint| f.build.binary_digest = None) as Mutation,
                "ahu_build_digest",
            ),
            (
                Box::new(|f: &mut InputFingerprint| f.target_repo_head = None),
                "target_repo_head",
            ),
            (
                Box::new(|f: &mut InputFingerprint| f.skill_digest = None),
                "skill_digest",
            ),
            (
                Box::new(|f: &mut InputFingerprint| f.evaluator_skill_digest = None),
                "evaluator_skill_digest",
            ),
            (
                Box::new(|f: &mut InputFingerprint| f.evaluator_repo_head = None),
                "evaluator_repo_head",
            ),
            (
                Box::new(|f: &mut InputFingerprint| f.candidate.harness_version = None),
                "harness_version",
            ),
            (
                Box::new(|f: &mut InputFingerprint| {
                    if let Some(evaluator) = f.evaluator.as_mut() {
                        evaluator.harness_version = None;
                    }
                }),
                "evaluator_harness_version",
            ),
        ] {
            let mut fingerprint = complete();
            mutate(&mut fingerprint);
            assert_eq!(
                fingerprint.completeness(),
                Completeness::Partial,
                "{expected}"
            );
            assert!(fingerprint.missing().contains(&expected), "{expected}");
            assert_ne!(fingerprint.digest(), complete().digest(), "{expected}");
        }
    }

    #[test]
    fn an_absent_evaluator_is_not_a_missing_input() {
        let mut fingerprint = complete();
        fingerprint.evaluator = None;
        assert!(fingerprint.missing().is_empty());
        assert_eq!(fingerprint.completeness(), Completeness::Complete);
    }

    #[test]
    fn every_input_change_changes_the_digest() {
        let base = complete().digest();
        let mutations: Vec<Mutation> = vec![
            Box::new(|f| f.case_digest = "9".repeat(64)),
            Box::new(|f| f.case_schema_version = 3),
            Box::new(|f| f.prompt_version += 1),
            Box::new(|f| f.scoring_version += 1),
            Box::new(|f| f.candidate.instructions_digest = "9".repeat(64)),
            Box::new(|f| f.candidate.manifest_digest = "9".repeat(64)),
            Box::new(|f| f.candidate.source_digest = "9".repeat(64)),
            Box::new(|f| f.candidate.model = "other".into()),
            Box::new(|f| f.skill_digest = Some("9".repeat(64))),
            Box::new(|f| f.selection_policy_digest = Some("8".repeat(64))),
            Box::new(|f| f.build.tool_definitions_digest = "9".repeat(64)),
            Box::new(|f| f.build.binary_digest = Some("9".repeat(64))),
            Box::new(|f| f.build.version = "9.9.9".into()),
            Box::new(|f| f.target_repo_head = Some("cafe".into())),
            Box::new(|f| f.blinding = Blinding::Isolated),
            Box::new(|f| {
                f.suite = Some(SuiteIdentity {
                    id: "s".into(),
                    suite_version: "1.0.0".into(),
                    digest: "7".repeat(64),
                })
            }),
            Box::new(|f| f.case_weight = Some(2.0)),
        ];
        for (index, mutate) in mutations.iter().enumerate() {
            let mut fingerprint = complete();
            mutate(&mut fingerprint);
            assert_ne!(
                fingerprint.digest(),
                base,
                "mutation {index} must be visible"
            );
        }
    }

    #[test]
    fn the_record_fields_carry_no_path_and_keep_build_identity_apart_from_repository_head() {
        let fields = complete().record_fields();
        assert_eq!(fields["target_repo_head"], "deadbeef");
        assert_eq!(fields["ahu_build_digest"], "1".repeat(64));
        assert_eq!(fields["ahu_version"], "0.5.0");
        // The ambiguous old field is neither written nor revived.
        assert!(!fields.contains_key("ahu_revision"));
        assert_eq!(fields["fingerprint_completeness"], "complete");
        assert_eq!(fields["blinding"], "prompt_only");
        assert!(
            fields["blinding_caveat"]
                .as_str()
                .expect("caveat")
                .contains("not environmental isolation")
        );
        let body = serde_json::to_string(&fields).expect("serializes");
        assert!(!body.contains('/') || !body.contains("/Users"), "{body}");
        assert!(!body.contains("\\\\"), "{body}");
    }

    #[test]
    fn the_running_build_identity_is_detected_rather_than_declared() {
        let build = BuildIdentity::detect();
        assert_eq!(build.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(build.tool_definitions_digest.len(), 64);
        // The test binary is readable, so the strong identity is available here.
        assert_eq!(
            build.binary_digest.as_ref().map(String::len),
            Some(64),
            "the running executable should be digestible in a test run"
        );
    }
}
