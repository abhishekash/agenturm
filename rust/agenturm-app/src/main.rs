use agenturm_app::{
    AgenturmApp, Command, discover, execute_attention, execute_backup, execute_backup_workspace,
    execute_cockpit_with_extensions, execute_collect, execute_content_stdout, execute_evals,
    execute_ingest, execute_inspect_ui, execute_investigation, execute_payload_stdout,
    execute_prune, execute_query, execute_restore_workspace, execute_retention,
    execute_selected_review, execute_service, execute_span_jsonl_import, execute_summary,
    execute_task_stdout, execute_trace_import, execute_trace_receiver, execute_trace_run,
    execute_watch, parse_os_command, render, with_private_stdout,
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
        #[cfg(target_os = "linux")]
        Command::InstallAdapter(options) => match agenturm_app::execute_adapter_install(&options) {
            Ok(path) => println!("{path}"),
            Err(message) => {
                eprintln!("adapter install failed: {message}");
                return ExitCode::FAILURE;
            }
        },
        #[cfg(target_os = "linux")]
        Command::Broker(options) => {
            return match agenturm_app::execute_broker(&options) {
                Ok(code) => code,
                Err(message) => {
                    eprintln!("broker failed: {message}");
                    ExitCode::from(message.exit_code())
                }
            };
        }
        Command::Attention(options) => {
            if let Err(message) = execute_attention(&options, std::io::stdout().lock()) {
                eprintln!("attention failed: {message}");
                return ExitCode::FAILURE;
            }
        }
        Command::Host(options) => {
            if let Err(message) = agenturm_app::execute_host(&options, std::io::stdout().lock()) {
                eprintln!("host failed: {message}");
                return ExitCode::FAILURE;
            }
        }
        #[cfg(target_os = "linux")]
        Command::Window(options) => {
            return match agenturm_app::execute_new_window(&options) {
                Ok(code) => code,
                Err(message) => {
                    eprintln!("cockpit failed: {message}");
                    ExitCode::FAILURE
                }
            };
        }
        #[cfg(target_os = "linux")]
        Command::RemoteCockpit(options) => {
            if let Err(message) = agenturm_app::execute_remote_cockpit(&options) {
                eprintln!("cockpit failed: {message}");
                return ExitCode::FAILURE;
            }
        }
        Command::Task(options) => {
            if let Err(message) = execute_task_stdout(&options) {
                eprintln!("task failed: {message}");
                return ExitCode::from(message.exit_code());
            }
        }
        Command::Retention(options) => match execute_retention(&options) {
            Ok(output) => print!("{output}"),
            Err(message) => {
                eprintln!("retention failed: {message}");
                return ExitCode::FAILURE;
            }
        },
        Command::Summary(options) => match execute_summary(&options) {
            Ok(output) => print!("{output}"),
            Err(message) => {
                eprintln!("summary failed: {message}");
                return ExitCode::FAILURE;
            }
        },
        Command::Investigate(options) => {
            if let Err(message) =
                with_private_stdout(|output| execute_investigation(&options, output))
            {
                eprintln!("investigate failed: {message}");
                return ExitCode::from(2);
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
        Command::ImportJsonl(options) => match execute_span_jsonl_import(&options) {
            Ok(summary) => print!("{summary}"),
            Err(message) => {
                eprintln!("span JSONL import failed: {message}");
                return ExitCode::FAILURE;
            }
        },
        Command::Evals(options) => match execute_evals(&options) {
            Ok(result) => print!("{result}"),
            Err(message) => {
                eprintln!("evaluation failed: {message}");
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
            selection,
            output,
        } => match execute_selected_review(&query, &selection, &output) {
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
            if let Err(message) = execute_content_stdout(&options) {
                eprintln!("content preview failed: {message}");
                return ExitCode::FAILURE;
            }
        }
        Command::TracePayload(options) => {
            if let Err(message) = execute_payload_stdout(&options) {
                eprintln!("telemetry payload failed: {message}");
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
