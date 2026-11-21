use agenturm_app::{
    AgenturmApp, Command, discover, execute_backup, execute_collect, execute_content,
    execute_ingest, execute_inspect_ui, execute_prune, execute_query, execute_service,
    execute_watch, parse_command, render,
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
        Command::Inspect(options) => {
            if let Err(message) = execute_inspect_ui(&options) {
                eprintln!("inspect failed: {message}");
                return ExitCode::FAILURE;
            }
        }
        Command::Ingest(options) => match execute_ingest(&options) {
            Ok(summary) => print!("{summary}"),
            Err(message) => {
                eprintln!("ingest failed: {message}");
                return ExitCode::FAILURE;
            }
        },
        Command::Query(options) => match execute_query(&options) {
            Ok(result) => println!("{result}"),
            Err(message) => {
                eprintln!("query failed: {message}");
                return ExitCode::FAILURE;
            }
        },
        Command::Backup { db, output } => {
            if let Err(message) = execute_backup(&db, &output) {
                eprintln!("backup failed: {message}");
                return ExitCode::FAILURE;
            }
            println!("Saved backup to {}", output.display());
        }
        Command::Prune { db, before_unix_ns } => match execute_prune(&db, before_unix_ns) {
            Ok(summary) => print!("{summary}"),
            Err(message) => {
                eprintln!("retention failed: {message}");
                return ExitCode::FAILURE;
            }
        },
        Command::Content(options) => {
            if let Err(message) = execute_content(&options, &mut std::io::stdout().lock()) {
                eprintln!("Pi content failed: {message}");
                return ExitCode::FAILURE;
            }
        }
        Command::Discover {
            sessions_per_harness,
            follow,
        } => match discover(sessions_per_harness, follow) {
            Ok(config) => println!("{config}"),
            Err(message) => {
                eprintln!("discovery failed: {message}");
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
        Command::Service(action) => match execute_service(&action) {
            Ok(message) => print!("{message}"),
            Err(message) => {
                eprintln!("service command failed: {message}");
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
