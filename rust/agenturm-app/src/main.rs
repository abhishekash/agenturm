use agenturm_app::{AgenturmApp, Command, execute_ingest, parse_command, render};
use std::process::ExitCode;

fn main() -> ExitCode {
    let command = match parse_command(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };

    match command {
        Command::Ingest(options) => match execute_ingest(&options) {
            Ok(summary) => print!("{summary}"),
            Err(message) => {
                eprintln!("ingest failed: {message}");
                return ExitCode::FAILURE;
            }
        },
        command => {
            if command == Command::Status {
                let mut app = AgenturmApp::empty_composition();
                let snapshot = app.refresh();
                debug_assert!(snapshot.failures.is_empty());
            }
            print!("{}", render(command));
        }
    }
    ExitCode::SUCCESS
}
