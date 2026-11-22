use agenturm_app::{
    AgenturmApp, Command, discover, execute_attention, execute_backup, execute_backup_workspace,
    execute_cockpit_with_extensions, execute_collect, execute_content, execute_ingest,
    execute_inspect_ui, execute_prune, execute_query, execute_restore_workspace, execute_review,
    execute_service, execute_task, execute_trace_import, execute_trace_receiver, execute_trace_run,
    execute_watch, parse_os_command, render,
};
use std::io::IsTerminal;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.is_empty() && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        args.push("cockpit".into());
    }
    let command = match parse_os_command(&args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };

    match command {
        Command::Attention(options) => {
            if let Err(message) = execute_attention(&options, std::io::stdout().lock()) {
                eprintln!("attention failed: {message}");
                return ExitCode::FAILURE;
            }
        }
        Command::Task(options) => {
            if let Err(message) = execute_task(&options, std::io::stdout().lock()) {
                eprintln!("task failed: {message}");
                return ExitCode::FAILURE;
            }
        }
        Command::Cockpit { db, extensions } => {
            if let Err(message) = execute_cockpit_with_extensions(db.as_deref(), extensions) {
                eprintln!("cockpit failed: {message}");
                return ExitCode::FAILURE;
            }
        }
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
        Command::ServeTraces(options) => {
            if let Err(message) = execute_trace_receiver(&options) {
                eprintln!("trace receiver failed: {message}");
                return ExitCode::FAILURE;
            }
        }
        Command::Run(options) => {
            return match execute_trace_run(&options) {
                Ok(code) => code,
                Err(message) => {
                    eprintln!("run failed: {message}");
                    ExitCode::FAILURE
                }
            };
        }
        Command::ImportTraces(options) => match execute_trace_import(&options) {
            Ok(summary) => print!("{summary}"),
            Err(message) => {
                eprintln!("trace import failed: {message}");
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
        Command::Review {
            query,
            turn_id,
            worker_id,
        } => match execute_review(&query, &turn_id, worker_id.as_deref()) {
            Ok(result) => println!("{result}"),
            Err(message) => {
                eprintln!("review failed: {message}");
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
        Command::BackupWorkspace { db, output } => {
            if let Err(message) = execute_backup_workspace(&db, &output) {
                eprintln!("workspace backup failed: {message}");
                return ExitCode::FAILURE;
            }
            println!("Saved workspace backup to {}", output.display());
        }
        Command::RestoreWorkspace { bundle, output } => {
            if let Err(message) = execute_restore_workspace(&bundle, &output) {
                eprintln!("workspace restore failed: {message}");
                return ExitCode::FAILURE;
            }
            println!(
                "Restored workspace history to {}",
                output.join("history.sqlite").display()
            );
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
                eprintln!("content preview failed: {message}");
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
