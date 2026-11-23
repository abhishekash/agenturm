//! Recognize Codex's conventional table and dotted-key MCP declarations.
//! The scanner understands string/comment boundaries, skips values, and marks
//! unsupported MCP syntax partial. It never returns configuration keys below
//! the server name except an explicitly declared `enabled` boolean.

use super::valid_name;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Default)]
enum Enabled {
    #[default]
    Unspecified,
    Reported(bool),
    Ambiguous,
}

pub(super) fn declarations(raw: &str) -> (BTreeMap<String, Option<bool>>, bool) {
    let mut servers = BTreeMap::<String, Enabled>::new();
    let mut declared_tables = BTreeSet::new();
    let mut table = Vec::new();
    let mut lex = Lex::default();
    let mut partial = false;
    for line in raw.lines() {
        let text = line.trim_start();
        if lex.at_statement() && !matches!(text.as_bytes().first(), None | Some(b'#')) {
            if let Some(header) = text.strip_prefix('[') {
                let array = header.starts_with('[');
                let first = if array { &header[1..] } else { header };
                let Some((keys, tail)) = key_path(first) else {
                    partial |= first.contains("mcp_servers");
                    table.clear();
                    if lex.scan(line).is_err() {
                        return (BTreeMap::new(), true);
                    }
                    continue;
                };
                let suffix = if array { "]]" } else { "]" };
                if tail
                    .strip_prefix(suffix)
                    .is_none_or(|rest| !end_or_comment(rest))
                {
                    return (BTreeMap::new(), true);
                }
                if keys.first().is_some_and(|key| key == "mcp_servers") {
                    if array || !declared_tables.insert(keys.clone()) {
                        partial = true;
                    }
                    if !array && let Some(name) = keys.get(1) {
                        if valid_name(name) {
                            servers.entry(name.clone()).or_default();
                        } else {
                            partial = true;
                        }
                    }
                }
                table = keys;
            } else if let Some((fields, tail)) = key_path(text) {
                let mut path = table.clone();
                path.extend(fields);
                if path.first().is_some_and(|key| key == "mcp_servers") {
                    let value = tail.strip_prefix('=').map(str::trim_start);
                    match &path[..] {
                        [_, name, field, rest @ ..] if valid_name(name) && value.is_some() => {
                            let enabled = servers.entry(name.clone()).or_default();
                            if field == "enabled" && rest.is_empty() {
                                let literal = value.unwrap_or_default();
                                let reported = [("true", true), ("false", false)]
                                    .into_iter()
                                    .find_map(|(token, value)| {
                                        literal
                                            .strip_prefix(token)
                                            .filter(|rest| end_or_comment(rest))
                                            .map(|_| value)
                                    });
                                *enabled = match (*enabled, reported) {
                                    (Enabled::Unspecified, Some(value)) => Enabled::Reported(value),
                                    _ => {
                                        partial = true;
                                        Enabled::Ambiguous
                                    }
                                };
                            }
                        }
                        _ => partial = true,
                    }
                }
            } else if text.starts_with("mcp_servers")
                || table.first().is_some_and(|key| key == "mcp_servers")
            {
                partial = true;
            }
        }
        if lex.scan(line).is_err() {
            return (BTreeMap::new(), true);
        }
    }
    if !lex.at_statement() {
        return (BTreeMap::new(), true);
    }
    let names = servers
        .into_iter()
        .map(|(server, enabled)| {
            (
                server,
                match enabled {
                    Enabled::Unspecified | Enabled::Ambiguous => None,
                    Enabled::Reported(value) => Some(value),
                },
            )
        })
        .collect();
    (names, partial)
}

fn end_or_comment(value: &str) -> bool {
    let value = value.trim_start();
    value.is_empty() || value.starts_with('#')
}

fn key_path(mut input: &str) -> Option<(Vec<String>, &str)> {
    let mut keys = Vec::new();
    loop {
        let (key, tail) = key(input.trim_start())?;
        keys.push(key);
        if keys.len() > 16 {
            return None;
        }
        let tail = tail.trim_start();
        let Some(next) = tail.strip_prefix('.') else {
            return Some((keys, tail));
        };
        input = next;
    }
}

fn key(input: &str) -> Option<(String, &str)> {
    let raw = input.as_bytes();
    let start = *raw.first()?;
    if matches!(start, b'\'' | b'"') {
        let mut index = 1;
        while let Some(&byte) = raw.get(index) {
            if byte == b'\\' && start == b'"' {
                index += 2;
                continue;
            }
            if byte == start {
                let value = if start == b'"' {
                    serde_json::from_str(&input[..=index]).ok()?
                } else {
                    input[1..index].to_owned()
                };
                return Some((value, &input[index + 1..]));
            }
            index += 1;
        }
        return None;
    }
    let end = raw
        .iter()
        .position(|byte| !byte.is_ascii_alphanumeric() && !b"_-".contains(byte))
        .unwrap_or(raw.len());
    (end != 0).then(|| (input[..end].to_owned(), &input[end..]))
}

#[derive(Default)]
struct Lex {
    string: Option<(u8, bool)>,
    nested: Vec<u8>,
}

impl Lex {
    fn at_statement(&self) -> bool {
        self.string.is_none() && self.nested.is_empty()
    }

    fn scan(&mut self, line: &str) -> Result<(), ()> {
        let bytes = line.as_bytes();
        let mut index = 0;
        while let Some(&byte) = bytes.get(index) {
            if let Some((quote, multiline)) = self.string {
                if byte == b'\\' && quote == b'"' {
                    index += 2;
                } else if byte == quote && (!multiline || triple(bytes, index, quote)) {
                    self.string = None;
                    index += if multiline { 3 } else { 1 };
                    if multiline {
                        let extra = bytes[index..]
                            .iter()
                            .take_while(|&&byte| byte == quote)
                            .count();
                        if extra > 2 {
                            return Err(());
                        }
                        index += extra;
                    }
                } else {
                    index += 1;
                }
                continue;
            }
            match byte {
                b'#' => break,
                b'"' | b'\'' => {
                    let multiline = triple(bytes, index, byte);
                    self.string = Some((byte, multiline));
                    index += if multiline { 3 } else { 1 };
                    continue;
                }
                b'[' => self.nested.push(b']'),
                b'{' => self.nested.push(b'}'),
                b']' | b'}' if self.nested.pop() != Some(byte) => return Err(()),
                _ => {}
            }
            if self.nested.len() > 64 {
                return Err(());
            }
            index += 1;
        }
        if self.string.is_some_and(|(_, multiline)| !multiline) {
            return Err(());
        }
        Ok(())
    }
}

fn triple(bytes: &[u8], position: usize, quote: u8) -> bool {
    bytes.get(position..position + 3) == Some(&[quote; 3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_tables_and_dotted_keys_without_labels_from_values() {
        let raw = r#"
            # [mcp_servers.comment]
            mcp_servers.dot.command = "private"
            [other]
            nested = [
                "[mcp_servers.argument]",
                { arbitrary = "never display" },
            ]
            script = """
            [mcp_servers.embedded]
            enabled = false
            """
            literal = '''
            [mcp_servers.embedded_literal]
            '''
            ["mcp_servers".'server.one']
            command = "secret-prog"
            enabled = false
            [mcp_servers.two.env]
            TOKEN = "private secret"
        "#;
        let (names, partial) = declarations(raw);
        assert!(!partial);
        assert_eq!(
            names,
            BTreeMap::from([
                ("dot".into(), None),
                ("server.one".into(), Some(false)),
                ("two".into(), None),
            ])
        );
    }

    #[test]
    fn conflicting_enabled_or_unsupported_shape_has_partial_coverage() {
        let (names, partial) = declarations(
            "[mcp_servers]\ninline = {}\n[mcp_servers.server]\nenabled = true\nenabled = false\n",
        );
        assert!(partial);
        assert_eq!(names, BTreeMap::from([("server".into(), None)]));
        assert_eq!(
            declarations("[mcp_servers.open]\nsecret = \"\"\"\n[mcp_servers.embedded]"),
            (BTreeMap::new(), true)
        );
    }
}
