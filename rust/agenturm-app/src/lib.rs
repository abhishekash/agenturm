//! Rust application composition boundary.
//!
//! This is intentionally an early shell: the product UI and production
//! harness/storage composition are not wired yet. Bundled and independently
//! installed extensions will be registered here through the kernel ports.

use agenturm_kernel::{Kernel, Snapshot};

pub struct AgenturmApp {
    kernel: Kernel,
}

impl AgenturmApp {
    pub fn new(kernel: Kernel) -> Self {
        Self { kernel }
    }

    pub fn empty_composition() -> Self {
        Self::new(Kernel::new())
    }

    pub fn refresh(&mut self) -> Snapshot {
        self.kernel.refresh()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Help,
    Version,
    Status,
}

pub fn parse_command(args: &[String]) -> Result<Command, String> {
    match args {
        [] => Ok(Command::Help),
        [arg] if arg == "--help" || arg == "-h" || arg == "help" => Ok(Command::Help),
        [arg] if arg == "--version" || arg == "-V" || arg == "version" => Ok(Command::Version),
        [arg] if arg == "status" => Ok(Command::Status),
        _ => Err("usage: agenturm [--help|--version|status]".into()),
    }
}

pub fn render(command: Command) -> String {
    match command {
        Command::Help => concat!(
            "Agenturm · Rust application (early composition shell)\n",
            "\n",
            "Usage: agenturm [--help|--version|status]\n",
            "\n",
            "The full-screen interface and production harness/storage wiring are not yet available.\n",
        )
        .into(),
        Command::Version => format!("agenturm {}\n", env!("CARGO_PKG_VERSION")),
        Command::Status => concat!(
            "Agenturm Rust kernel: ready\n",
            "Registered harnesses: 0\n",
            "Registered capabilities: 0\n",
            "Registered UI contributions: 0\n",
            "Production storage: not connected\n",
        )
        .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_application_refresh_is_explicitly_empty() {
        let mut app = AgenturmApp::empty_composition();
        let snapshot = app.refresh();
        assert!(snapshot.observations.is_empty());
        assert!(snapshot.metrics.is_empty());
        assert!(snapshot.panes.is_empty());
        assert!(snapshot.failures.is_empty());
    }

    #[test]
    fn command_parser_has_stable_help_version_and_status_commands() {
        assert_eq!(parse_command(&[]).unwrap(), Command::Help);
        assert_eq!(
            parse_command(&["--version".into()]).unwrap(),
            Command::Version
        );
        assert_eq!(parse_command(&["status".into()]).unwrap(), Command::Status);
        assert!(parse_command(&["unknown".into()]).is_err());
    }

    #[test]
    fn status_output_does_not_claim_unwired_components() {
        let status = render(Command::Status);
        assert!(status.contains("Registered harnesses: 0"));
        assert!(status.contains("Production storage: not connected"));
    }
}
