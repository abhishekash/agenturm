//! Shared bounds and filtering for private text.

pub fn bounded_text_preview(text: String, max_bytes: usize) -> Result<String, String> {
    if text.is_empty() || text.len() > max_bytes {
        return Err("Content is unavailable within the preview text limit".into());
    }
    let lower = text.to_ascii_lowercase();
    let known_credential = [
        "private key",
        "sk-",
        "sk_live_",
        "sk_test_",
        "rk_live_",
        "rk_test_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "akia",
        "aiza",
        "xoxb-",
        "xoxa-",
        "xoxp-",
        "xoxr-",
        "xoxs-",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    let password_command = lower.lines().any(|line| {
        let mut words = line.split_ascii_whitespace();
        words.next() == Some("mysql") && words.any(|word| word.starts_with("-p") && word.len() > 2)
    });
    let bearer_value = lower.split_ascii_whitespace().any(|word| word == "bearer");
    let assigned_credential = lower.lines().any(|line| {
        (line.contains('=') || line.contains(':'))
            && [
                "authorization",
                "password",
                "passwd",
                "token",
                "secret",
                "credential",
                "api_key",
                "api-key",
                "apikey",
            ]
            .iter()
            .any(|marker| line.contains(marker))
    });
    let uri_credential = lower.split_ascii_whitespace().any(|word| {
        word.split_once("://")
            .and_then(|(_, tail)| tail.split('/').next())
            .is_some_and(|authority| authority.contains(':') && authority.contains('@'))
    });
    if known_credential || password_command || bearer_value || assigned_credential || uri_credential
    {
        return Err(
            "Prompt preview withheld because it contains possible credential material".into(),
        );
    }
    let text: String = text
        .chars()
        .filter(|&character| {
            matches!(character, '\n' | '\t')
                || (!character.is_control()
                    && !matches!(
                        character,
                        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
                    ))
        })
        .collect();
    if text.is_empty() {
        return Err("Content is unavailable within the preview text limit".into());
    }
    Ok(text)
}
