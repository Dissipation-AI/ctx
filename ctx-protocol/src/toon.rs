//! A small encoder for [TOON](https://github.com/toon-format/toon) (Token-Oriented Object
//! Notation), written against the format's spec. ctx only emits TOON and never parses it.
//!
//! - Objects are `key: value` lines; nested objects are indented by two spaces.
//! - Arrays of primitives are inline: `tags[3]: a,b,c`.
//! - Arrays of objects that share the same primitive-valued keys are tables:
//!   `commands[2]{number,command}:` then one comma-separated row per object.
//! - Other arrays are lists of `- ` items.
//! - Strings are quoted only when they'd otherwise be ambiguous.

use serde_json::Value;

/// Encode a JSON value as a TOON document.
pub fn encode(value: &Value) -> String {
    let mut out = String::new();
    match value {
        Value::Object(map) => write_object(&mut out, map, 0),
        Value::Array(items) => write_array(&mut out, None, items, 0),
        other => out.push_str(&primitive(other)),
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_object(out: &mut String, map: &serde_json::Map<String, Value>, depth: usize) {
    for (key, value) in map {
        match value {
            Value::Object(inner) => {
                indent(out, depth);
                out.push_str(&key_text(key));
                out.push(':');
                out.push('\n');
                write_object(out, inner, depth + 1);
            }
            Value::Array(items) => write_array(out, Some(key), items, depth),
            other => {
                indent(out, depth);
                out.push_str(&key_text(key));
                out.push_str(": ");
                out.push_str(&primitive(other));
                out.push('\n');
            }
        }
    }
}

fn is_primitive(v: &Value) -> bool {
    !matches!(v, Value::Array(_) | Value::Object(_))
}

/// Field names shared by every object in `items`, in first-seen order, when all values are
/// primitives. `None` if the array isn't tabular.
fn table_fields(items: &[Value]) -> Option<Vec<String>> {
    let first = items.first()?.as_object()?;
    let fields: Vec<String> = first.keys().cloned().collect();
    if fields.is_empty() {
        return None;
    }
    for item in items {
        let obj = item.as_object()?;
        if obj.len() != fields.len() || !fields.iter().all(|f| obj.get(f).is_some_and(is_primitive)) {
            return None;
        }
    }
    Some(fields)
}

fn write_array(out: &mut String, key: Option<&str>, items: &[Value], depth: usize) {
    indent(out, depth);
    if let Some(k) = key {
        out.push_str(&key_text(k));
    }
    out.push_str(&format!("[{}]", items.len()));
    if items.iter().all(is_primitive) {
        out.push(':');
        if !items.is_empty() {
            out.push(' ');
            let cells: Vec<String> = items.iter().map(primitive).collect();
            out.push_str(&cells.join(","));
        }
        out.push('\n');
        return;
    }
    if let Some(fields) = table_fields(items) {
        let header: Vec<String> = fields.iter().map(|f| key_text(f)).collect();
        out.push_str(&format!("{{{}}}:\n", header.join(",")));
        for item in items {
            let obj = item.as_object().expect("checked by table_fields");
            let cells: Vec<String> = fields.iter().map(|f| primitive(&obj[f])).collect();
            indent(out, depth + 1);
            out.push_str(&cells.join(","));
            out.push('\n');
        }
        return;
    }
    out.push_str(":\n");
    for item in items {
        match item {
            Value::Object(map) => {
                // First field on the dash line, the rest indented beneath it.
                let mut first = true;
                for (k, v) in map {
                    if first {
                        indent(out, depth + 1);
                        out.push_str("- ");
                        first = false;
                        match v {
                            Value::Object(inner) => {
                                out.push_str(&key_text(k));
                                out.push_str(":\n");
                                write_object(out, inner, depth + 3);
                            }
                            Value::Array(arr) => {
                                let mut sub = String::new();
                                write_array(&mut sub, Some(k), arr, 0);
                                // Re-indent nested lines under the dash.
                                let mut lines = sub.lines();
                                if let Some(l) = lines.next() {
                                    out.push_str(l);
                                    out.push('\n');
                                }
                                for l in lines {
                                    indent(out, depth + 2);
                                    out.push_str(l);
                                    out.push('\n');
                                }
                            }
                            other => {
                                out.push_str(&key_text(k));
                                out.push_str(": ");
                                out.push_str(&primitive(other));
                                out.push('\n');
                            }
                        }
                    } else {
                        let mut single = serde_json::Map::new();
                        single.insert(k.clone(), v.clone());
                        write_object(out, &single, depth + 2);
                    }
                }
                if first {
                    indent(out, depth + 1);
                    out.push_str("-\n");
                }
            }
            Value::Array(arr) => {
                let mut sub = String::new();
                write_array(&mut sub, None, arr, 0);
                let mut lines = sub.lines();
                if let Some(l) = lines.next() {
                    indent(out, depth + 1);
                    out.push_str("- ");
                    out.push_str(l);
                    out.push('\n');
                }
                for l in lines {
                    indent(out, depth + 2);
                    out.push_str(l);
                    out.push('\n');
                }
            }
            other => {
                indent(out, depth + 1);
                out.push_str("- ");
                out.push_str(&primitive(other));
                out.push('\n');
            }
        }
    }
}

fn key_text(key: &str) -> String {
    let plain = !key.is_empty()
        && key.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    if plain { key.to_string() } else { quote(key) }
}

fn primitive(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => string_text(s),
        _ => unreachable!("primitive() is only called on primitives"),
    }
}

fn looks_like_number(s: &str) -> bool {
    let t = s.strip_prefix('-').unwrap_or(s);
    !t.is_empty()
        && t.chars().all(|c| c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-')
        && t.chars().next().is_some_and(|c| c.is_ascii_digit())
}

fn string_text(s: &str) -> String {
    let needs_quotes = s.is_empty()
        || s != s.trim()
        || matches!(s, "true" | "false" | "null")
        || looks_like_number(s)
        || s.starts_with('-')
        || s.chars().any(|c| matches!(c, ',' | ':' | '"' | '\\' | '[' | ']' | '{' | '}' | '#') || c.is_control());
    if needs_quotes { quote(s) } else { s.to_string() }
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tables_for_uniform_records() {
        let v = json!({
            "commands": [
                {"number": 13, "command": "cargo build", "where": "c@macbook:~/code/site", "exit_status": 0},
                {"number": 14, "command": "cargo test -p auth", "where": "c@macbook:~/code/site", "exit_status": 101}
            ]
        });
        assert_eq!(
            encode(&v),
            "commands[2]{number,command,where,exit_status}:\n  13,cargo build,\"c@macbook:~/code/site\",0\n  14,cargo test -p auth,\"c@macbook:~/code/site\",101"
        );
    }

    #[test]
    fn nested_and_primitives() {
        let v = json!({"schema": 1, "tab": {"number": 4, "name": "Dev server"}, "tags": ["a", "b"], "empty": [], "note": null, "text": "a, b"});
        assert_eq!(
            encode(&v),
            "schema: 1\ntab:\n  number: 4\n  name: Dev server\ntags[2]: a,b\nempty[0]:\nnote: null\ntext: \"a, b\""
        );
    }

    #[test]
    fn mixed_arrays_become_lists() {
        let v = json!({"commands": [{"number": 1, "output": ["x", "y"]}]});
        assert_eq!(encode(&v), "commands[1]:\n  - number: 1\n    output[2]: x,y");
    }

    #[test]
    fn ambiguous_strings_are_quoted() {
        assert_eq!(string_text("42"), "\"42\"");
        assert_eq!(string_text("true"), "\"true\"");
        assert_eq!(string_text(""), "\"\"");
        assert_eq!(string_text("- x"), "\"- x\"");
        assert_eq!(string_text("line\nbreak"), "\"line\\nbreak\"");
        assert_eq!(string_text("plain words"), "plain words");
    }
}
