//! Changing one value in a JSON file's text and nothing else (#114): a profile written by hand keeps its order, its
//! indentation, its line endings and every other value as it was, so its diff is just the change. The text is read
//! into where each value starts and ends; a change replaces, adds or removes one span, and a new value is written in
//! the file's own style - its indent unit, its line ending, and an array's items on one line when its others are.

use serde_json::Value;

/// A new object is written on one line when it's at most this long and its values are plain.
const COMPACT_MOST: usize = 120;
/// The indent unit when the file shows none.
const INDENT: &str = "  ";

/// Where a value is in the text, and what's in it.
#[derive(Debug)]
enum Node {
    Object { start: usize, end: usize, members: Vec<(String, Node)> },
    Array { start: usize, end: usize, items: Vec<Node> },
    Plain { start: usize, end: usize },
}

impl Node {
    fn span(&self) -> (usize, usize) {
        match self {
            Self::Object { start, end, .. } | Self::Array { start, end, .. } | Self::Plain { start, end } => {
                (*start, *end)
            }
        }
    }

    fn member(&self, key: &str) -> Option<&Node> {
        match self {
            Self::Object { members, .. } => members.iter().find(|(name, _)| name == key).map(|(_, node)| node),
            _ => None,
        }
    }
}

/// A small reader that notes where each value is.
struct Reader<'a> {
    text: &'a str,
    at: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.at += 1;
        }
        if self.text[self.at..].starts_with('\u{feff}') {
            self.at += '\u{feff}'.len_utf8();
            self.space();
        }
    }

    fn fail<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{what} at byte {}", self.at))
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        self.space();
        if self.peek() != Some(byte) {
            return self.fail(&format!("expected '{}'", byte as char));
        }
        self.at += 1;
        Ok(())
    }

    fn string(&mut self) -> Result<String, String> {
        let start = self.at;
        self.expect(b'"')?;
        loop {
            match self.peek() {
                None => return self.fail("an unfinished string"),
                Some(b'\\') => self.at += 2,
                Some(b'"') => {
                    self.at += 1;
                    break;
                }
                Some(_) => self.at += 1,
            }
        }
        serde_json::from_str(&self.text[start..self.at]).map_err(|error| error.to_string())
    }

    fn value(&mut self) -> Result<Node, String> {
        self.space();
        let start = self.at;
        match self.peek() {
            Some(b'{') => {
                self.at += 1;
                let mut members = Vec::new();
                self.space();
                if self.peek() == Some(b'}') {
                    self.at += 1;
                    return Ok(Node::Object { start, end: self.at, members });
                }
                loop {
                    self.space();
                    let key = self.string()?;
                    self.expect(b':')?;
                    members.push((key, self.value()?));
                    self.space();
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Node::Object { start, end: self.at, members });
                        }
                        _ => return self.fail("expected ',' or '}'"),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.peek() == Some(b']') {
                    self.at += 1;
                    return Ok(Node::Array { start, end: self.at, items });
                }
                loop {
                    items.push(self.value()?);
                    self.space();
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Node::Array { start, end: self.at, items });
                        }
                        _ => return self.fail("expected ',' or ']'"),
                    }
                }
            }
            Some(b'"') => {
                self.string()?;
                Ok(Node::Plain { start, end: self.at })
            }
            Some(_) => {
                while self.peek().is_some_and(|byte| byte.is_ascii_alphanumeric() || b"+-.".contains(&byte)) {
                    self.at += 1;
                }
                if self.at == start {
                    return self.fail("expected a value");
                }
                Ok(Node::Plain { start, end: self.at })
            }
            None => self.fail("expected a value"),
        }
    }
}

fn read(text: &str) -> Result<Node, String> {
    let mut reader = Reader { text, at: 0 };
    let root = reader.value()?;
    reader.space();
    if reader.at != text.len() {
        return reader.fail("more after the value");
    }
    Ok(root)
}

/// How the file is written: its line ending and its indent unit.
struct Style {
    eol: &'static str,
    unit: String,
}

fn style(text: &str) -> Style {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let unit = text
        .lines()
        .map(|line| &line[..line.len() - line.trim_start_matches([' ', '\t']).len()])
        .find(|indent| !indent.is_empty())
        .map_or_else(
            || INDENT.to_string(),
            |indent| if indent.starts_with('\t') { "\t".into() } else { indent.into() },
        );
    Style { eol, unit }
}

/// The whitespace a line starts with, for the line `at` is on.
fn line_indent(text: &str, at: usize) -> &str {
    let start = text[..at].rfind('\n').map_or(0, |found| found + 1);
    let line = &text[start..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// How an object is written on one line: `{ "a": 1 }` or `{"a":1}`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OneLine {
    Spaced,
    Tight,
}

fn plain(value: &Value) -> bool {
    match value {
        Value::Object(_) => false,
        Value::Array(items) => items.iter().all(|item| !item.is_object() && !item.is_array()),
        _ => true,
    }
}

fn one_line(value: &Value, how: OneLine) -> String {
    let text = |value: &Value| value.to_string();
    match (value, how) {
        (Value::Object(members), OneLine::Spaced) if members.is_empty() => "{}".into(),
        (Value::Object(members), OneLine::Spaced) => {
            let inner: Vec<String> = members
                .iter()
                .map(|(key, value)| format!("{}: {}", text(&Value::from(key.as_str())), one_line(value, how)))
                .collect();
            format!("{{ {} }}", inner.join(", "))
        }
        (Value::Array(items), OneLine::Spaced) => {
            let inner: Vec<String> = items.iter().map(|item| one_line(item, how)).collect();
            format!("[{}]", inner.join(", "))
        }
        _ => text(value),
    }
}

/// A value as the file would have it, its first line where it's put and its others indented from `indent`: an
/// object with plain values on one line when it's short (`how`, or as `force` says), else one member a line.
fn render(value: &Value, indent: &str, style: &Style, force: Option<Option<OneLine>>) -> String {
    let inner = format!("{indent}{}", style.unit);
    match value {
        Value::Object(members) if !members.is_empty() => {
            let compact = one_line(value, OneLine::Spaced);
            let fits = members.values().all(plain) && compact.len() <= COMPACT_MOST;
            match force {
                Some(Some(how)) => return one_line(value, how),
                None if fits => return compact,
                _ => {}
            }
            let lines: Vec<String> = members
                .iter()
                .map(|(key, value)| {
                    format!("{inner}{}: {}", Value::from(key.as_str()), render(value, &inner, style, None))
                })
                .collect();
            format!("{{{eol}{}{eol}{indent}}}", lines.join(&format!(",{}", style.eol)), eol = style.eol)
        }
        Value::Array(items) if !items.is_empty() && !plain(value) => {
            let fits = |item: &Value| {
                item.as_object().is_some_and(|members| members.values().all(plain))
                    && one_line(item, OneLine::Spaced).len() <= COMPACT_MOST
            };
            let alike = Some(items.iter().all(fits).then_some(OneLine::Spaced));
            let lines: Vec<String> =
                items.iter().map(|item| format!("{inner}{}", render(item, &inner, style, alike))).collect();
            format!("[{eol}{}{eol}{indent}]", lines.join(&format!(",{}", style.eol)), eol = style.eol)
        }
        _ => one_line(value, OneLine::Spaced),
    }
}

/// The node at `path` (object keys from the root).
fn at<'a>(root: &'a Node, path: &[&str]) -> Result<&'a Node, String> {
    path.iter().try_fold(root, |node, key| node.member(key).ok_or_else(|| format!("no \"{key}\"")))
}

fn splice(text: &str, (start, end): (usize, usize), with: &str) -> String {
    format!("{}{with}{}", &text[..start], &text[end..])
}

/// Sets `key` of the object at `path` to `value`: its value replaced where it's there, else a member added after the
/// object's last one (an object along the path that isn't there is added too).
pub fn set_member(text: &str, path: &[&str], key: &str, value: &Value) -> Result<String, String> {
    let root = read(text)?;
    let style = style(text);
    let mut depth = 0;
    let mut node = &root;
    while depth < path.len() {
        match node.member(path[depth]) {
            Some(found) => node = found,
            None => break,
        }
        depth += 1;
    }
    let (key, value) = if depth < path.len() {
        let mut wrapped = serde_json::Map::new();
        wrapped.insert(key.to_string(), value.clone());
        let mut inner = Value::Object(wrapped);
        for name in path[depth + 1..].iter().rev() {
            let mut outer = serde_json::Map::new();
            outer.insert((*name).to_string(), inner);
            inner = Value::Object(outer);
        }
        (path[depth], inner)
    } else {
        (key, value.clone())
    };
    let Node::Object { start, end, members } = node else { return Err(format!("\"{key}\"'s place isn't an object")) };
    if let Some((_, old)) = members.iter().find(|(name, _)| name == key) {
        let indent = line_indent(text, old.span().0);
        return Ok(splice(text, old.span(), &render(&value, indent, &style, None)));
    }
    let name = Value::from(key).to_string();
    match members.last() {
        Some((_, last)) => {
            let spread = text[*start..*end].contains('\n');
            let first_key = text[*start + 1..].find('"').map_or(*start, |found| *start + 1 + found);
            let indent = if spread { line_indent(text, first_key).to_string() } else { String::new() };
            let gap = if spread { format!("{}{indent}", style.eol) } else { " ".into() };
            let added = format!(",{gap}{name}: {}", render(&value, &indent, &style, None));
            let after = last.span().1;
            Ok(splice(text, (after, after), &added))
        }
        None => {
            let outer = line_indent(text, *start);
            let inner = format!("{outer}{}", style.unit);
            let added = format!(
                "{{{eol}{inner}{name}: {}{eol}{outer}}}",
                render(&value, &inner, &style, None),
                eol = style.eol
            );
            Ok(splice(text, (*start, *end), &added))
        }
    }
}

/// The array at `path`.
fn array<'a>(root: &'a Node, path: &[&str]) -> Result<(usize, usize, &'a [Node]), String> {
    match at(root, path)? {
        Node::Array { start, end, items } => Ok((*start, *end, items)),
        _ => Err(format!("\"{}\" isn't a list", path.last().unwrap_or(&""))),
    }
}

/// How the array's items are written: on one line each (spaced or tight), or spread.
fn items_look(text: &str, items: &[Node]) -> Option<Option<OneLine>> {
    let sample = items.iter().find(|item| matches!(item, Node::Object { .. }))?;
    let (start, end) = sample.span();
    let written = &text[start..end];
    if written.contains('\n') {
        return Some(None);
    }
    Some(Some(if written.starts_with("{ ") || written == "{}" { OneLine::Spaced } else { OneLine::Tight }))
}

/// Adds `value` at the end of the array at `path`.
pub fn push_item(text: &str, path: &[&str], value: &Value) -> Result<String, String> {
    let root = read(text)?;
    let style = style(text);
    let (start, end, items) = array(&root, path)?;
    let force = items_look(text, items);
    match items.last() {
        Some(last) => {
            let spread = text[start..end].contains('\n');
            let indent = if spread { line_indent(text, items[0].span().0).to_string() } else { String::new() };
            let gap = if spread { format!("{}{indent}", style.eol) } else { " ".into() };
            let added = format!(",{gap}{}", render(value, &indent, &style, force));
            Ok(splice(text, (last.span().1, last.span().1), &added))
        }
        None => {
            let outer = line_indent(text, start);
            let inner = format!("{outer}{}", style.unit);
            let added = format!("[{eol}{inner}{}{eol}{outer}]", render(value, &inner, &style, force), eol = style.eol);
            Ok(splice(text, (start, end), &added))
        }
    }
}

/// Replaces item `index` of the array at `path` with `value`, written as the item was.
pub fn replace_item(text: &str, path: &[&str], index: usize, value: &Value) -> Result<String, String> {
    let root = read(text)?;
    let style = style(text);
    let (_, _, items) = array(&root, path)?;
    let item = items.get(index).ok_or_else(|| format!("no item {index}"))?;
    let (start, end) = item.span();
    let force = if text[start..end].contains('\n') {
        Some(None)
    } else {
        Some(Some(if text[start..end].starts_with("{ ") { OneLine::Spaced } else { OneLine::Tight }))
    };
    Ok(splice(text, (start, end), &render(value, line_indent(text, start), &style, force)))
}

/// Takes item `index` out of the array at `path`, with the comma and the space before it.
pub fn remove_item(text: &str, path: &[&str], index: usize) -> Result<String, String> {
    let root = read(text)?;
    let (start, end, items) = array(&root, path)?;
    let item = items.get(index).ok_or_else(|| format!("no item {index}"))?;
    let span = match (index, items.len()) {
        (_, 1) => (start + 1, end - 1),
        (0, _) => (item.span().0, items[1].span().0),
        _ => (items[index - 1].span().1, item.span().1),
    };
    Ok(splice(text, span, ""))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const LF: &str = "{\n  \"id\": \"shop\",\n  \"apps\": [\n    { \"key\": \"api\" }\n  ],\n  \"watch\": \"src\"\n}\n";

    fn parsed(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn a_new_member_goes_after_the_last_one_in_the_files_own_indent_and_line_ending() {
        let source = json!([{ "name": "qa", "server": "qa-sql.example.com,1433", "auth": "sql", "user": "reader" }]);
        let lf = set_member(LF, &[], "databaseSources", &source).unwrap();
        assert_eq!(
            lf,
            "{\n  \"id\": \"shop\",\n  \"apps\": [\n    { \"key\": \"api\" }\n  ],\n  \"watch\": \"src\",\n  \
             \"databaseSources\": [\n    { \"name\": \"qa\", \"server\": \"qa-sql.example.com,1433\", \"auth\": \
             \"sql\", \"user\": \"reader\" }\n  ]\n}\n"
        );
        let crlf_tabs = LF.replace("\n", "\r\n").replace("  ", "\t");
        let written = set_member(&crlf_tabs, &[], "databaseSources", &source).unwrap();
        assert!(written.starts_with(&crlf_tabs[..crlf_tabs.len() - 5]), "the rest untouched: {written:?}");
        assert!(written.contains(",\r\n\t\"databaseSources\": [\r\n\t\t{ \"name\""), "{written:?}");
        assert!(!written.replace("\r\n", "").contains('\n'), "CRLF throughout");
        assert_eq!(parsed(&written)["databaseSources"], source);
        let four = LF.replace("  ", "    ");
        let written = set_member(&four, &[], "keepPulled", &json!(2)).unwrap();
        assert!(written.ends_with("\"watch\": \"src\",\n    \"keepPulled\": 2\n}\n"), "{written}");
    }

    #[test]
    fn a_members_value_is_replaced_in_place_and_nothing_else_moves() {
        let written = set_member(LF, &[], "watch", &json!(["src", "lib"])).unwrap();
        assert_eq!(written, LF.replace("\"watch\": \"src\"", "\"watch\": [\"src\", \"lib\"]"));
        let nested = set_member("{}", &["projects", "shop"], "databaseSources", &json!([])).unwrap();
        assert_eq!(parsed(&nested), json!({ "projects": { "shop": { "databaseSources": [] } } }));
    }

    #[test]
    fn an_item_is_added_replaced_and_removed_as_its_neighbours_are_written() {
        let text = "{\r\n  \"databaseSources\": [\r\n    { \"name\": \"qa\", \"server\": \"a\" },\r\n    { \"name\": \"dev\", \"server\": \"b\" }\r\n  ]\r\n}";
        let pushed =
            push_item(text, &["databaseSources"], &json!({ "name": "staging", "connectionStringEnv": "X" })).unwrap();
        assert!(
            pushed.contains("\"b\" },\r\n    { \"name\": \"staging\", \"connectionStringEnv\": \"X\" }\r\n  ]"),
            "{pushed:?}"
        );
        let replaced = replace_item(text, &["databaseSources"], 0, &json!({ "name": "qa", "server": "c" })).unwrap();
        assert_eq!(replaced, text.replace("\"server\": \"a\"", "\"server\": \"c\""));
        let first = remove_item(text, &["databaseSources"], 0).unwrap();
        assert_eq!(first, "{\r\n  \"databaseSources\": [\r\n    { \"name\": \"dev\", \"server\": \"b\" }\r\n  ]\r\n}");
        let last = remove_item(text, &["databaseSources"], 1).unwrap();
        assert_eq!(last, "{\r\n  \"databaseSources\": [\r\n    { \"name\": \"qa\", \"server\": \"a\" }\r\n  ]\r\n}");
        let none = remove_item(&last, &["databaseSources"], 0).unwrap();
        assert_eq!(none, "{\r\n  \"databaseSources\": []\r\n}");
        let again = push_item(&none, &["databaseSources"], &json!({ "name": "qa" })).unwrap();
        assert_eq!(again, "{\r\n  \"databaseSources\": [\r\n    { \"name\": \"qa\" }\r\n  ]\r\n}");
    }

    #[test]
    fn spread_and_tight_items_stay_spread_and_tight() {
        let spread = "{\n\t\"s\": [\n\t\t{\n\t\t\t\"name\": \"qa\"\n\t\t}\n\t]\n}\n";
        let pushed = push_item(spread, &["s"], &json!({ "name": "dev", "user": "r" })).unwrap();
        assert_eq!(
            pushed,
            "{\n\t\"s\": [\n\t\t{\n\t\t\t\"name\": \"qa\"\n\t\t},\n\t\t{\n\t\t\t\"name\": \"dev\",\n\t\t\t\"user\": \"r\"\n\t\t}\n\t]\n}\n"
        );
        let tight = "{\"s\":[{\"name\":\"qa\"}]}";
        assert_eq!(
            push_item(tight, &["s"], &json!({ "name": "dev" })).unwrap(),
            "{\"s\":[{\"name\":\"qa\"}, {\"name\":\"dev\"}]}"
        );
    }

    #[test]
    fn a_file_that_isnt_json_is_never_changed() {
        assert!(set_member("{ \"a\": 1, }", &[], "b", &json!(1)).is_err());
        assert!(push_item("{ \"a\": 1 }", &["a"], &json!(1)).unwrap_err().contains("isn't a list"));
        assert!(
            set_member("\u{feff}{ \"a\": \"x\\\"y\" }", &[], "b", &json!(true)).unwrap().ends_with(", \"b\": true }")
        );
    }
}
