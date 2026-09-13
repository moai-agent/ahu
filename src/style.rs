//! Semantic terminal styling. Only this module emits terminal escapes.

use std::io::IsTerminal;
use std::sync::OnceLock;

use crate::util::display_safe_block;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy)]
pub enum Role {
    Agent,
    Runtime,
    Warning,
    Error,
    Gap,
    Drift,
    Success,
    Hint,
    Heading,
}

#[derive(Debug, Clone, Copy)]
pub struct Style {
    enabled: bool,
}

impl Style {
    pub const fn plain() -> Self {
        Self { enabled: false }
    }

    /// Always/never override the environment; auto honors NO_COLOR and stdout.
    pub fn detect(choice: Option<ColorChoice>) -> Self {
        Self::resolve(
            choice,
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var_os("TERM").is_some_and(|term| term == "dumb"),
            std::io::stdout().is_terminal(),
        )
    }

    pub const fn resolve(
        choice: Option<ColorChoice>,
        no_color: bool,
        dumb: bool,
        stdout_is_terminal: bool,
    ) -> Self {
        let enabled = match choice {
            Some(ColorChoice::Always) => true,
            Some(ColorChoice::Never) => false,
            Some(ColorChoice::Auto) | None => !no_color && !dumb && stdout_is_terminal,
        };
        Self { enabled }
    }

    /// Wrap a complete safe span, never inserting escapes within its text.
    /// Unsafe input is neutralized and returned without styling as a backstop
    /// for callers that forgot to escape a repository-controlled value.
    pub fn paint(self, role: Role, text: &str) -> String {
        let safe = display_safe_block(text);
        if !self.enabled || safe != text || text.is_empty() {
            return safe;
        }
        let sgr = match role {
            Role::Agent => "1;36",
            Role::Runtime => "36",
            Role::Warning => "33",
            Role::Error => "1;31",
            Role::Gap => "1;33",
            Role::Drift => "35",
            Role::Success => "32",
            Role::Hint => "2",
            Role::Heading => "1",
        };
        format!("\x1b[{sgr}m{text}\x1b[0m")
    }
}

static STDOUT_STYLE: OnceLock<Style> = OnceLock::new();

/// Set the process's output policy once, before command dispatch.
pub fn configure(choice: Option<ColorChoice>) {
    let _ = STDOUT_STYLE.set(Style::detect(choice));
}

/// Library callers remain plain unless the application configures styling.
pub fn stdout() -> Style {
    STDOUT_STYLE.get().copied().unwrap_or_else(Style::plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_color_precedes_environment_and_detection() {
        for no_color in [false, true] {
            for dumb in [false, true] {
                for terminal in [false, true] {
                    assert!(
                        Style::resolve(Some(ColorChoice::Always), no_color, dumb, terminal).enabled
                    );
                    assert!(
                        !Style::resolve(Some(ColorChoice::Never), no_color, dumb, terminal).enabled
                    );
                    assert_eq!(
                        Style::resolve(Some(ColorChoice::Auto), no_color, dumb, terminal).enabled,
                        !no_color && !dumb && terminal
                    );
                    assert_eq!(
                        Style::resolve(None, no_color, dumb, terminal).enabled,
                        !no_color && !dumb && terminal
                    );
                }
            }
        }
    }

    #[test]
    fn unsafe_input_is_neutralized_without_wrapping_it() {
        let style = Style::resolve(Some(ColorChoice::Always), false, false, true);
        assert_eq!(style.paint(Role::Agent, "x\x1b[2Jy"), "x\\x1b[2Jy");
        assert_eq!(style.paint(Role::Agent, "x\u{202e}y"), "x\\u{202e}y");
        let safe = crate::util::display_safe("x\x1b[2J\u{202e}y");
        assert_eq!(
            style.paint(Role::Agent, &safe),
            format!("\x1b[1;36m{safe}\x1b[0m")
        );
    }
}
