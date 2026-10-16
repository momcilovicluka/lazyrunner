//! The field reference in projects/README.md, as Markdown, from profile.schema.json: one table per part of a
//! profile, a row per field with its type, whether it is required, and the schema's description, default and
//! example. A test keeps the guide in step; `LAZYRUNNER_WRITE_REFERENCE=1 cargo test -p lazyrunner-core reference`
//! rewrites it after a schema change.

use serde_json::Value;

use crate::json::{field, list, text, truthy};

pub const START_MARKER: &str =
    "<!-- reference: made from profile.schema.json by projects\\Update-Reference.ps1 - change the schema, not this -->";
pub const END_MARKER: &str = "<!-- end of reference -->";
const APP_PARTS: [&str; 4] = ["dotnetApp", "npmApp", "serveVariant", "buildVariant"];

pub fn reference(schema: &Value) -> String {
    let defs = field(schema, "$defs");
    let mut lines: Vec<String> = Vec::new();
    let mut required: Vec<String> = list(field(schema, "required")).into_iter().map(text).collect();
    required.extend(list(field(field(schema, "else"), "required")).into_iter().map(text));
    let description = text(field(schema, "description"));
    add_table(&mut lines, "The profile", &description, field(schema, "properties"), &required, defs);
    for name in APP_PARTS {
        let def = field(defs, name);
        let required: Vec<String> = list(field(def, "required")).into_iter().map(text).collect();
        let (title, description) = (text(field(def, "title")), text(field(def, "description")));
        add_table(&mut lines, &title, &description, field(def, "properties"), &required, defs);
    }
    lines.join("\n").trim_end().to_string()
}

/// The guide with its reference section replaced; `None` when the markers are missing.
pub fn replace_section(guide: &str, reference: &str) -> Option<String> {
    let start = guide.find(START_MARKER)? + START_MARKER.len();
    let end = guide.find(END_MARKER).filter(|&end| end >= start)?;
    Some(format!("{}\n\n{reference}\n\n{}", &guide[..start], &guide[end..]))
}

/// The reference section of the guide, as written there.
pub fn section(guide: &str) -> Option<&str> {
    let start = guide.find(START_MARKER)? + START_MARKER.len();
    let end = guide.find(END_MARKER).filter(|&end| end >= start)?;
    Some(guide[start..end].trim())
}

fn add_table(
    lines: &mut Vec<String>,
    title: &str,
    description: &str,
    properties: &Value,
    required: &[String],
    defs: &Value,
) {
    lines.extend([format!("#### {title}"), String::new(), description.to_string(), String::new()]);
    lines.extend(["| Field | Type | Required | What it does |".to_string(), "|---|---|---|---|".to_string()]);
    for (name, node) in properties.as_object().into_iter().flatten() {
        add_row(lines, name, node, required.contains(name), defs);
        let target = resolve(node, defs);
        if text(field(target, "type")) == "object" && truthy(field(target, "properties")) {
            let child_required: Vec<String> = list(field(target, "required")).into_iter().map(text).collect();
            for (child, child_node) in field(target, "properties").as_object().into_iter().flatten() {
                add_row(lines, &format!("{name}.{child}"), child_node, child_required.contains(child), defs);
            }
        }
    }
    lines.push(String::new());
}

fn ref_name(node: &Value) -> Option<&str> {
    let reference =
        node.get("$ref").or_else(|| list(field(node, "allOf")).first().and_then(|first| first.get("$ref")))?;
    reference.as_str()?.rsplit('/').next()
}

fn resolve<'a>(node: &'a Value, defs: &'a Value) -> &'a Value {
    ref_name(node).map_or(node, |name| field(defs, name))
}

fn type_name(node: &Value) -> String {
    let constant = field(node, "const");
    if !constant.is_null() {
        return format!("`{constant}`");
    }
    if let Some(name) = ref_name(node) {
        return match name {
            "relativePath" => "path",
            "appKey" => "app key",
            "icon" => "character",
            _ => "object",
        }
        .into();
    }
    if text(field(node, "type")) == "array" {
        let items = field(node, "items");
        if items.is_null() {
            return "array".into();
        }
        if truthy(field(items, "oneOf")) {
            return "array of objects".into();
        }
        return format!("array of {}s", type_name(items));
    }
    text(field(node, "type"))
}

fn add_row(lines: &mut Vec<String>, name: &str, node: &Value, required: bool, defs: &Value) {
    let target = resolve(node, defs);
    let mut description = if truthy(field(node, "description")) {
        text(field(node, "description"))
    } else {
        text(field(target, "description"))
    };
    let default = if field(node, "default").is_null() { field(target, "default") } else { field(node, "default") };
    let examples = if truthy(field(node, "examples")) { field(node, "examples") } else { field(target, "examples") };
    if !default.is_null() {
        description.push_str(&format!(" Default: `{}`.", text(default)));
    }
    if truthy(examples)
        && field(node, "const").is_null()
        && let Some(first) = list(examples).first()
    {
        description.push_str(&format!(" Example: `{first}`."));
    }
    let yes = if required { "yes" } else { "" };
    lines.push(format!("| `{name}` | {} | {yes} | {} |", type_name(node), description.trim().replace('|', "\\|")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::check::FIELDS;
    use crate::profiletools::SCHEMA;

    const GUIDE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../projects/README.md");

    fn generated() -> String {
        reference(&serde_json::from_str(SCHEMA).unwrap())
    }

    #[test]
    fn the_guides_field_reference_is_made_from_the_schema() {
        let guide = std::fs::read_to_string(GUIDE).unwrap().replace("\r\n", "\n");
        if std::env::var_os("LAZYRUNNER_WRITE_REFERENCE").is_some() {
            std::fs::write(GUIDE, replace_section(&guide, &generated()).expect("the guide has both markers")).unwrap();
            return;
        }
        assert_eq!(
            section(&guide).expect("the guide has both markers"),
            generated(),
            "projects/README.md is out of date: LAZYRUNNER_WRITE_REFERENCE=1 cargo test -p lazyrunner-core reference"
        );
    }

    #[test]
    fn it_lists_exactly_the_fields_the_loader_knows() {
        let mut fields: Vec<String> = generated()
            .lines()
            .filter_map(|line| line.strip_prefix("| `"))
            .filter_map(|rest| rest.split('`').next())
            .map(|name| name.rsplit('.').next().unwrap_or(name).to_string())
            .collect();
        fields.sort();
        fields.dedup();
        assert_eq!(fields, FIELDS.all());
    }

    #[test]
    fn replacing_the_section_keeps_the_rest_of_the_guide() {
        let guide = format!("intro\n{START_MARKER}\n\nold\n\n{END_MARKER}\nrest\n");
        let updated = replace_section(&guide, "new").unwrap();
        assert_eq!(updated, format!("intro\n{START_MARKER}\n\nnew\n\n{END_MARKER}\nrest\n"));
        assert_eq!(section(&updated), Some("new"));
    }
}
