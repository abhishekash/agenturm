use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;

const BOOTSTRAP: &str = include_str!("python_auto.py");
const TARGET_HINT: &str =
    "Automatic instrumentation requires Python 3.11+ followed by a script or -m module";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PythonInstrumentation {
    module: bool,
    service: String,
    capture_content: bool,
}

impl PythonInstrumentation {
    pub(super) fn parse(
        program: &OsStr,
        arguments: &[OsString],
        service: Option<String>,
        capture_content: bool,
    ) -> Result<Self, String> {
        let executable = Path::new(program).file_name().and_then(OsStr::to_str);
        if !executable.is_some_and(is_python) {
            return Err(TARGET_HINT.into());
        }
        let module = arguments.first().is_some_and(|argument| argument == "-m");
        let target = arguments.get(usize::from(module)).ok_or(TARGET_HINT)?;
        if target.is_empty() || target.as_encoded_bytes().starts_with(b"-") {
            return Err(TARGET_HINT.into());
        }
        if module
            && !target.to_str().is_some_and(|name| {
                name.split('.').all(|part| {
                    part.bytes()
                        .next()
                        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
                        && part
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                })
            })
        {
            return Err("Python module name must contain dot-separated identifiers".into());
        }
        let service = service
            .or_else(|| std::env::var("OTEL_SERVICE_NAME").ok())
            .unwrap_or_else(|| "my-agent".into());
        let service = service.trim();
        if service.is_empty()
            || service.chars().count() > 128
            || service.chars().any(char::is_control)
        {
            return Err(
                "Python instrumentation service name must be 1–128 printable characters".into(),
            );
        }
        Ok(Self {
            module,
            service: service.into(),
            capture_content,
        })
    }

    pub(super) fn configure(
        &self,
        command: &mut Command,
        arguments: &[OsString],
        launch_session: &str,
    ) {
        command
            .args(["-P", "-c", BOOTSTRAP])
            .arg(if self.module { "module" } else { "script" })
            .arg(if self.capture_content {
                "capture"
            } else {
                "metadata"
            })
            .arg(&self.service)
            .arg(launch_session)
            .args(&arguments[usize::from(self.module)..]);
    }
}

fn is_python(program: &str) -> bool {
    matches!(program, "python" | "python3")
        || program
            .strip_prefix("python3.")
            .filter(|minor| minor.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|minor| minor.parse::<u8>().ok())
            .is_some_and(|minor| minor >= 11)
}
