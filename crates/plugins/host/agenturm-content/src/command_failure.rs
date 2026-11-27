use super::RecordedInvocation;
use serde_json::Value;

/// Classify only the selected, byte-proven completion. Contradictory or
/// malformed terminal evidence cannot establish a failure.
pub(super) fn confirmed_failure(
    invocation: &RecordedInvocation,
    status: Option<&Value>,
    exit: Option<&Value>,
    result: Option<&Value>,
) -> bool {
    let status = match status {
        None => "",
        Some(Value::String(status)) => status.as_str(),
        _ => return false,
    };
    let exit = match exit {
        None => None,
        Some(value) => match value.as_i64() {
            Some(code) => Some(code),
            None => return false,
        },
    };
    let error = match result.and_then(|value| value.get("isError")) {
        None => None,
        Some(Value::Bool(error)) => Some(*error),
        _ => return false,
    };
    let denied = status.eq_ignore_ascii_case("denied") || status.eq_ignore_ascii_case("declined");
    let failed = denied || matches!(status, "failed" | "error" | "failure") || error == Some(true);
    let succeeded =
        matches!(status, "completed" | "success" | "succeeded" | "ok") || error == Some(false);
    if (failed && (succeeded || exit == Some(0)))
        || (succeeded && exit.is_some_and(|code| code != 0))
        || (!status.is_empty() && !failed && !succeeded)
    {
        return false;
    }
    if exit == Some(1) && !denied && error != Some(true) && informational_exit_one(invocation) {
        return false;
    }
    failed || exit.is_some_and(|code| code != 0)
}

fn informational_exit_one(invocation: &RecordedInvocation) -> bool {
    let mut words = match invocation {
        RecordedInvocation::Text(text) => match shlex::split(text) {
            Some(words) if lexical_command(text, &words) => words,
            _ => return false,
        },
        RecordedInvocation::Argv(words) => words.clone(),
    };
    for _ in 0..4 {
        let Some(program) = words.first().map(|word| executable(word)) else {
            return false;
        };
        if matches!(program, "sh" | "bash" | "zsh" | "dash" | "ksh") {
            let [_, flags @ .., text] = words.as_slice() else {
                return false;
            };
            let Some((last, preceding)) = flags.split_last() else {
                return false;
            };
            if !matches!(last.as_str(), "-c" | "-lc" | "-cl")
                || !preceding.iter().all(|flag| {
                    matches!(flag.as_str(), "-l" | "--login" | "--noprofile" | "--norc")
                })
            {
                return false;
            }
            let Some(parsed) = shlex::split(text).filter(|parsed| lexical_command(text, parsed))
            else {
                return false;
            };
            words = parsed;
            continue;
        }
        if words.len() < 2 || words.iter().any(|word| word == "--check") {
            return false;
        }
        return matches!(program, "grep" | "rg" | "diff")
            || (program == "git"
                && words[1] == "diff"
                && words[2..]
                    .iter()
                    .any(|word| matches!(word.as_str(), "--quiet" | "--exit-code")));
    }
    false
}

fn lexical_command(text: &str, words: &[String]) -> bool {
    words
        .first()
        .is_some_and(|word| matches!(executable(word), "sh" | "bash" | "zsh" | "dash" | "ksh"))
        || !text.contains(['&', '|', ';', '`', '$', '#', '\n', '\r', '<', '>'])
}

fn executable(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_recognized_informational_invocations_suppress_exit_one() {
        for (invocation, expected) in [
            (RecordedInvocation::Text("diff old new".into()), false),
            (RecordedInvocation::Text("git diff --quiet".into()), false),
            (
                RecordedInvocation::Text("git diff --quiet --check".into()),
                true,
            ),
            (
                RecordedInvocation::Text("rg missing file || other".into()),
                true,
            ),
            (
                RecordedInvocation::Text("sh -c 'grep missing file'".into()),
                false,
            ),
            (
                RecordedInvocation::Text("sh -c 'rg missing file' modifier".into()),
                true,
            ),
            (
                RecordedInvocation::Argv(vec!["rg".into(), "literal|pipe".into(), "file".into()]),
                false,
            ),
        ] {
            assert_eq!(
                confirmed_failure(&invocation, Some(&json!("failed")), Some(&json!(1)), None),
                expected
            );
        }
        let rg = RecordedInvocation::Text("rg missing file".into());
        assert!(confirmed_failure(&rg, Some(&json!("denied")), None, None));
        assert!(confirmed_failure(&rg, None, Some(&json!(2)), None));
        assert!(confirmed_failure(
            &rg,
            None,
            Some(&json!(1)),
            Some(&json!({"isError": true}))
        ));
        assert!(!confirmed_failure(
            &rg,
            None,
            Some(&json!(2)),
            Some(&json!({"isError": false}))
        ));
        assert!(!confirmed_failure(
            &rg,
            Some(&json!("failed")),
            Some(&json!(0)),
            None
        ));
        assert!(!confirmed_failure(
            &rg,
            Some(&json!("started")),
            Some(&json!(2)),
            None
        ));
        assert!(!confirmed_failure(&rg, None, Some(&json!("2")), None));
        assert!(!confirmed_failure(&rg, None, None, None));
    }
}
