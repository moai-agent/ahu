//! The interactive launcher: agent selection, the prompt composer, and the
//! first-run setup UI.
//!
//! Two rules shape this module. Pasting must never launch anything — a separate
//! explicit submit action does that. And the resolved harness and model are
//! shown before submission, so nobody discovers after the fact which provider
//! their prompt went to.

use std::io::{BufRead, IsTerminal, Write};

use crate::agent::ResolvedAgent;
use crate::bail;
use crate::catalog;
use crate::config::{ContextHygiene, ProjectConfig};
use crate::selection;
use crate::util::Result;

/// Terminal input and output, injectable so the flow is testable.
pub struct Console<'a> {
    pub input: &'a mut dyn BufRead,
    pub output: &'a mut dyn Write,
    pub interactive: bool,
}

impl Console<'_> {
    pub fn say(&mut self, text: &str) -> Result<()> {
        self.output.write_all(text.as_bytes())?;
        self.output.flush()?;
        Ok(())
    }

    /// Read one line. `None` means end of input.
    pub fn read_line(&mut self) -> Result<Option<String>> {
        let mut buffer = String::new();
        let read = self.input.read_line(&mut buffer)?;
        if read == 0 {
            return Ok(None);
        }
        Ok(Some(buffer.trim_end_matches(['\n', '\r']).to_string()))
    }

    pub fn ask(&mut self, question: &str) -> Result<Option<String>> {
        self.say(question)?;
        self.read_line()
    }
}

/// Build a console over the real terminal.
pub fn stdio_console<'a>(stdin: &'a mut dyn BufRead, stdout: &'a mut dyn Write) -> Console<'a> {
    Console {
        input: stdin,
        output: stdout,
        interactive: std::io::stdin().is_terminal(),
    }
}

/// The line that ends the prompt composer.
pub const SUBMIT_SENTINEL: &str = ".";

/// What the composer produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composed {
    /// Agent name from an `@name` selector, if one was given.
    pub agent: Option<String>,
    pub prompt: String,
}

/// Read an optional `@agent` selector.
pub fn read_selector(
    console: &mut Console<'_>,
    agents: &[ResolvedAgent],
) -> Result<Option<String>> {
    if agents.is_empty() {
        console.say(
            "No ahu agents are registered in this repository.\n\
             Submitting without a selector uses the project's automatic selection.\n\
             Run `ahu onboard` to see native definitions that could be registered.\n\n",
        )?;
    } else {
        console.say("Agents in this repository:\n")?;
        for agent in agents {
            console.say(&format!(
                "  @{:<16} {} on {} / {}\n",
                agent.manifest.name,
                agent.manifest.version,
                agent.manifest.harness,
                agent.manifest.model
            ))?;
            if !agent.manifest.description.is_empty() {
                console.say(&format!("   {:>17} {}\n", "", agent.manifest.description))?;
            }
        }
        console.say("\n")?;
    }
    let answer = console.ask("Agent (e.g. @chris), or blank for automatic selection: ")?;
    let Some(answer) = answer else {
        return Ok(None);
    };
    let trimmed = answer.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let name = trimmed.strip_prefix('@').unwrap_or(trimmed);
    if name.is_empty() {
        bail!("`@` on its own is not an agent name.");
    }
    Ok(Some(name.to_string()))
}

/// Read a multiline prompt.
///
/// Nothing is submitted while this runs. A pasted block — however many lines —
/// only ends when the user types the sentinel on a line of its own or sends end
/// of input.
pub fn read_prompt(console: &mut Console<'_>) -> Result<Option<String>> {
    console.say(&format!(
        "\nTask prompt. Paste or type as many lines as you like.\n\
         Pasting does not submit. Finish with a line containing only `{SUBMIT_SENTINEL}`, \
         or end input.\n\
         Type `{SUBMIT_SENTINEL}cancel` on its own line to abandon it.\n\n"
    ))?;
    let mut lines: Vec<String> = Vec::new();
    while let Some(line) = console.read_line()? {
        if line.trim_end() == SUBMIT_SENTINEL {
            break;
        }
        if line.trim_end() == ".cancel" {
            return Ok(None);
        }
        lines.push(line);
    }
    let prompt = lines.join("\n");
    let prompt = prompt.trim_matches('\n').to_string();
    if prompt.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(prompt))
}

/// Ask a yes/no question. Anything but an explicit yes means no, and end of
/// input means no, so a non-answer never becomes a confirmation.
pub fn confirm(console: &mut Console<'_>, question: &str) -> Result<bool> {
    let answer = console.ask(question)?;
    Ok(matches!(
        answer
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("y") | Some("yes")
    ))
}

/// Ask the final submit question.
pub fn confirm_submit(console: &mut Console<'_>) -> Result<bool> {
    confirm(console, "\nSubmit this task? [y/N]: ")
}

/// First-run repository initialization.
///
/// The initializing user is establishing project policy for everyone, not a
/// personal profile. Cancellation writes nothing.
pub fn run_setup(console: &mut Console<'_>) -> Result<Option<ProjectConfig>> {
    if !console.interactive {
        bail!(
            "this repository has no ahu configuration yet, and ahu was not run interactively.\n\
             Initialization records the project-agreed harness and model order, so ahu will not \
             invent one. Run `ahu init` from a terminal.\n\
             Nothing was changed."
        );
    }
    console.say(
        "ahu is not initialized in this repository.\n\
         Setup records this project's agreed harness and model order. It applies to every ahu\n\
         user in the project; there are no personal overrides.\n\n\
         Harnesses in this build's compatibility catalog:\n",
    )?;
    for (index, harness) in catalog::HARNESSES.iter().enumerate() {
        let prerequisite = selection::check_prerequisite(harness.id);
        let availability = if !harness.adapter_available {
            "no ahu adapter in 0.1.1".to_string()
        } else if prerequisite.satisfied() {
            format!(
                "installed{}",
                prerequisite
                    .version
                    .as_deref()
                    .map(|v| format!(" ({v})"))
                    .unwrap_or_default()
            )
        } else {
            "not installed on this machine".to_string()
        };
        console.say(&format!(
            "  {}. {:<14} {:<12} [{availability}]\n",
            index + 1,
            harness.id,
            harness.display_name
        ))?;
    }
    console.say(
        "\nInstallation status is a local detail. It does not change the order you agree here,\n\
         and a machine missing a harness reports a diagnostic rather than picking another one.\n\n",
    )?;

    let answer = console.ask("Harness order, best first, by number (e.g. 1 or 2,1): ")?;
    let Some(answer) = answer else {
        return Ok(None);
    };
    if answer.trim().is_empty() {
        console.say("No selection made. Nothing was written.\n")?;
        return Ok(None);
    }
    let mut harness_preferences = Vec::new();
    for token in answer.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let index: usize = token.parse().map_err(|_| {
            crate::util::Error::new(format!("{token:?} is not one of the numbers listed."))
        })?;
        let harness = catalog::HARNESSES
            .get(index.wrapping_sub(1))
            .ok_or_else(|| {
                crate::util::Error::new(format!("{index} is not one of the numbers listed."))
            })?;
        if harness_preferences.contains(&harness.id.to_string()) {
            bail!("{} was listed twice.", harness.id);
        }
        harness_preferences.push(harness.id.to_string());
    }
    if harness_preferences.is_empty() {
        console.say("No selection made. Nothing was written.\n")?;
        return Ok(None);
    }

    let mut model_rankings = std::collections::BTreeMap::new();
    for harness_id in &harness_preferences {
        let models = catalog::models_for(harness_id);
        if models.is_empty() {
            console.say(&format!(
                "\nCatalog {} lists no models for {harness_id}; no ranking is recorded for it.\n",
                catalog::CATALOG_VERSION
            ))?;
            continue;
        }
        console.say(&format!("\nModels available for {harness_id}:\n"))?;
        for (index, model) in models.iter().enumerate() {
            console.say(&format!(
                "  {}. {:<22} {}\n      basis: {} (reviewed {})\n",
                index + 1,
                model.model,
                model.display_name,
                model.evaluation_basis,
                model.reviewed_on
            ))?;
        }
        console
            .say("\nThis order is the project's, not a claim about which model is smarter.\n")?;
        let suggested: Vec<String> = models.iter().map(|m| m.model.to_string()).collect();
        let answer = console.ask(&format!(
            "Model order for {harness_id} by number, or blank to accept the catalog order: "
        ))?;
        let Some(answer) = answer else {
            return Ok(None);
        };
        let chosen = if answer.trim().is_empty() {
            suggested
        } else {
            let mut chosen = Vec::new();
            for token in answer.split(',') {
                let token = token.trim();
                if token.is_empty() {
                    continue;
                }
                let index: usize = token.parse().map_err(|_| {
                    crate::util::Error::new(format!("{token:?} is not one of the numbers listed."))
                })?;
                let model = models.get(index.wrapping_sub(1)).ok_or_else(|| {
                    crate::util::Error::new(format!("{index} is not one of the numbers listed."))
                })?;
                if chosen.contains(&model.model.to_string()) {
                    bail!("{} was listed twice.", model.model);
                }
                chosen.push(model.model.to_string());
            }
            chosen
        };
        if chosen.is_empty() {
            bail!("no model was selected for {harness_id}.");
        }
        model_rankings.insert(harness_id.clone(), chosen);
    }

    let interval = loop {
        let answer = console.ask("\nContext hygiene review interval in days [7]: ")?;
        let Some(answer) = answer else {
            return Ok(None);
        };
        let trimmed = answer.trim();
        if trimmed.is_empty() {
            break 7;
        }
        match trimmed.parse::<u32>() {
            Ok(value) if value >= 1 => break value,
            _ => console.say("Enter a whole number of days, at least 1.\n")?,
        }
    };

    let config = ProjectConfig {
        schema_version: crate::config::SUPPORTED_SCHEMA_VERSION,
        harness_preferences,
        model_selection: "project-ranked".to_string(),
        catalog_version: catalog::CATALOG_VERSION.to_string(),
        model_rankings,
        context_hygiene: ContextHygiene {
            review_on_first_load: true,
            review_interval_days: interval,
        },
    };

    console.say("\nConfiguration to be written to .agents/ahu/config.toml:\n\n")?;
    console.say(&crate::config::render(&config))?;
    console.say(
        "\nThis file is usable immediately, committed or not. ahu will not stage, commit, or\n\
         push it; sharing it with the project is yours to do.\n",
    )?;
    if !confirm(console, "\nSave it? [y/N]: ")? {
        console.say("Cancelled. Nothing was written.\n")?;
        return Ok(None);
    }
    Ok(Some(config))
}
