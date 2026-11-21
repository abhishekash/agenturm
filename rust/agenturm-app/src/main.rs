use agenturm_app::{
    AgenturmApp, Command, execute_collect, execute_ingest, execute_inspect, execute_watch,
    parse_command, render,
};
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
        Command::Inspect(db) => match execute_inspect(&db) {
            Ok(snapshot) => match agenturm_ui::run(snapshot) {
                Ok(()) => {}
                Err(error) => {
                    eprintln!("Agenturm UI failed: {error}");
                    return ExitCode::FAILURE;
                }
            },
            Err(message) => {
                eprintln!("inspect failed: {message}");
                return ExitCode::FAILURE;
            }
        },
        Command::Ingest(options) => match execute_ingest(&options) {
            Ok(summary) => print!("{summary}"),
            Err(message) => {
                eprintln!("ingest failed: {message}");
                return ExitCode::FAILURE;
            }
        },
        Command::Collect(options) => {
            if let Some(interval) = options.watch_interval {
                match execute_watch(&options, interval) {
                    Ok(report) => {
                        println!("Stopped after {} collection cycle(s).", report.cycles);
                        if report.had_failures {
                            return ExitCode::FAILURE;
                        }
                    }
                    Err(message) => {
                        eprintln!("collection watch failed: {message}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                match execute_collect(&options) {
                    Ok(report) => {
                        print!("{}", report.summary);
                        if report.has_failures() {
                            return ExitCode::FAILURE;
                        }
                    }
                    Err(message) => {
                        eprintln!("collection failed: {message}");
                        return ExitCode::FAILURE;
                    }
                }
            }
        }
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
