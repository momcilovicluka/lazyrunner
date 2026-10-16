//! JSON the way lazyrunner's profiles and settings files are read: objects keep their key order, a missing or broken
//! file reads as nothing, and values are tested the way the PowerShell version did, so its checks port one for one
//! (an empty string, array or 0 counts as "not given"; lookups of names a person typed ignore case).

use std::io::{self, Read};
use std::path::Path;
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value};

/// How often a read is retried while another process rewrites the file.
const READ_ATTEMPTS: usize = 5;
const READ_RETRY: Duration = Duration::from_millis(40);
/// Profiles, settings and package.json files are small; anything bigger isn't one (or is a trap in an untrusted
/// repo, like a link to /dev/zero).
pub const MAX_FILE: u64 = 8 * 1024 * 1024;

/// A regular file's text (lossy UTF-8, no byte order mark), at most [`MAX_FILE`] bytes.
pub fn read_text(path: &Path) -> io::Result<String> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a file"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(MAX_FILE).read_to_end(&mut bytes)?;
    Ok(strip_bom(&String::from_utf8_lossy(&bytes)).to_string())
}

/// The file as JSON, or `None` when it's missing or doesn't parse (another process may be writing it: retried a few
/// times first).
pub fn read(path: &Path) -> Option<Value> {
    read_for_update(path).ok().flatten()
}

/// For a file about to be rewritten: `None` only when it doesn't exist; a file that can't be read or parsed is an
/// error, so its contents aren't replaced by an empty object.
pub fn read_for_update(path: &Path) -> io::Result<Option<Value>> {
    let mut last = None;
    for attempt in 0..READ_ATTEMPTS {
        if attempt > 0 {
            thread::sleep(READ_RETRY);
        }
        match read_text(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => last = Some(error),
            Ok(text) => match serde_json::from_str(&text) {
                Ok(value) => return Ok(Some(value)),
                Err(error) => last = Some(io::Error::new(io::ErrorKind::InvalidData, error)),
            },
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("unreadable")))
}

/// Text without a UTF-8 byte order mark (Windows editors add one).
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Whether a value counts as given: not null, false, 0, "" or an empty array (a one-item array counts as its item).
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(items) => match items.as_slice() {
            [] => false,
            [only] => truthy(only),
            _ => true,
        },
        Value::Object(_) => true,
    }
}

/// A value as text, for messages and for checks on what a person typed: null is "", true is "True", an array is
/// its items joined with spaces.
pub fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(text).collect::<Vec<_>>().join(" "),
        Value::Object(_) => "System.Collections.Hashtable".into(),
    }
}

/// A value as a list: null is empty, an array is its items, anything else is a list of one.
pub fn list(value: &Value) -> Vec<&Value> {
    match value {
        Value::Null => Vec::new(),
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    }
}

/// A field of an object (null when the value isn't an object or has no such field).
pub fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    value.get(name).unwrap_or(&NULL)
}

/// A field whose name is matched ignoring case (launch profile names, npm scripts).
pub fn field_ci<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.as_object()?.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, v)| v)
}

/// Whether an object has the key, ignoring case.
pub fn has_key_ci(object: &Value, name: &str) -> bool {
    field_ci(object, name).is_some()
}

/// A whole number (1, not 1.0 or "1").
pub fn is_integer(value: &Value) -> bool {
    value.as_i64().is_some() || value.as_u64().is_some()
}

/// The keys of an object, in their order (none for anything else).
pub fn keys(value: &Value) -> Vec<&str> {
    value.as_object().map(|object| object.keys().map(String::as_str).collect()).unwrap_or_default()
}

/// JSON with every object's keys sorted, so the same data always gives the same text.
pub fn sorted(value: &Value) -> String {
    fn sort(value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let mut keys: Vec<&String> = object.keys().collect();
                keys.sort();
                Value::Object(keys.into_iter().map(|key| (key.clone(), sort(&object[key]))).collect::<Map<_, _>>())
            }
            Value::Array(items) => Value::Array(items.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    sort(value).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truthy_follows_what_a_person_means_by_given() {
        for value in [json!(null), json!(false), json!(0), json!(""), json!([]), json!([""]), json!([0])] {
            assert!(!truthy(&value), "{value} is not given");
        }
        for value in [json!(true), json!(1), json!("x"), json!(["a", ""]), json!({}), json!(["a"])] {
            assert!(truthy(&value), "{value} is given");
        }
    }

    #[test]
    fn text_and_list_read_any_value() {
        assert_eq!(text(&json!(null)), "");
        assert_eq!(text(&json!(5)), "5");
        assert_eq!(text(&json!(["a", 2])), "a 2");
        assert_eq!(list(&json!("x")), [&json!("x")]);
        assert!(list(&json!(null)).is_empty());
    }

    #[test]
    fn lookups_by_a_typed_name_ignore_case() {
        let scripts = json!({ "Dev": "vite" });
        assert_eq!(field_ci(&scripts, "dev"), Some(&json!("vite")));
        assert!(!has_key_ci(&scripts, "serve"));
        assert_eq!(field(&json!("not an object"), "x"), &Value::Null);
    }

    #[test]
    fn sorted_json_ignores_key_order() {
        assert_eq!(
            sorted(&json!({ "b": 1, "a": { "d": [1], "c": 2 } })),
            sorted(&json!({ "a": { "c": 2, "d": [1] }, "b": 1 }))
        );
        assert_eq!(sorted(&json!({ "b": 1, "a": 2 })), r#"{"a":2,"b":1}"#);
    }

    #[test]
    fn a_missing_or_broken_file_reads_as_nothing() {
        let dir = std::env::temp_dir().join(format!("lazyrunner-json-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.json");
        assert_eq!(read(&file), None);
        std::fs::write(&file, "\u{feff}{ \"a\": 1 }").unwrap();
        assert_eq!(read(&file), Some(json!({ "a": 1 })), "a byte order mark is fine");
        std::fs::write(&file, "{ broken").unwrap();
        assert_eq!(read(&file), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
