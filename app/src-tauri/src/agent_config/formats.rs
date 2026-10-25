//! Edits to agent config files that keep everything else as the user wrote it: JSON and JSONC go
//! through a concrete syntax tree (comments, key order, indentation and unknown keys survive),
//! TOML through `toml_edit` (comments, key order, tables). Every function takes the file's text
//! and returns the new text; nothing here touches the disk.

use std::str::FromStr;

use jsonc_parser::cst::{CstInputValue, CstNode, CstObject, CstRootNode};
use jsonc_parser::ParseOptions;
use serde_json::Value;
use toml_edit::{DocumentMut, Item, Table};

type Res<T> = Result<T, String>;

const BOM: char = '\u{feff}';

fn split_bom(text: &str) -> (&str, &str) {
    match text.strip_prefix(BOM) {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    }
}

// ---------- JSON / JSONC ----------

/// The document as a value. Strict JSON takes the fast path (config files can be megabytes); a
/// file with comments or trailing commas (JSONC) goes through the tolerant parser. Empty = `{}`.
pub fn json_read(text: &str) -> Res<Value> {
    let (_, body) = split_bom(text);
    if body.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        return Ok(v);
    }
    let root = CstRootNode::parse(body, &ParseOptions::default()).map_err(|e| format!("not valid JSON: {e}"))?;
    root.to_serde_value().ok_or_else(|| "not valid JSON".to_string())
}

/// Only the named top-level keys of the document, as an object. Config files can be megabytes
/// of history around a few servers, so the strict path skips everything else without building it.
pub fn json_read_keys(text: &str, keys: &[&str]) -> Res<Value> {
    use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, Visitor};
    struct Pick<'a>(&'a [&'a str]);
    impl<'de> Visitor<'de> for Pick<'_> {
        type Value = serde_json::Map<String, Value>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a JSON object")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut out = serde_json::Map::new();
            while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
                if self.0.contains(&&*key) {
                    out.insert(key.into_owned(), map.next_value()?);
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            Ok(out)
        }
    }
    impl<'de> DeserializeSeed<'de> for Pick<'_> {
        type Value = serde_json::Map<String, Value>;
        fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_map(self)
        }
    }
    let (_, body) = split_bom(text);
    let mut de = serde_json::Deserializer::from_str(body);
    if let Ok(map) = Pick(keys).deserialize(&mut de).and_then(|m| de.end().map(|_| m)) {
        return Ok(Value::Object(map));
    }
    // Comments, trailing commas, an empty file, or a real error (reported by the full reader).
    match json_read(text)? {
        Value::Object(o) => Ok(Value::Object(o.into_iter().filter(|(k, _)| keys.contains(&k.as_str())).collect())),
        _ => Err("the top level of the file is not an object".into()),
    }
}

/// Whether `text` is a JSON document an agent would read (comments and trailing commas are fine
/// only where the file already had them: see [`json_is_strict`]).
pub fn json_check(text: &str) -> Res<()> {
    json_read(text).map(|_| ())
}

pub fn json_is_strict(text: &str) -> bool {
    serde_json::from_str::<Value>(split_bom(text).1).is_ok()
}

fn to_input(v: &Value) -> CstInputValue {
    match v {
        Value::Null => CstInputValue::Null,
        Value::Bool(b) => CstInputValue::Bool(*b),
        Value::Number(n) => CstInputValue::Number(n.to_string()),
        Value::String(s) => CstInputValue::String(s.clone()),
        Value::Array(a) => CstInputValue::Array(a.iter().map(to_input).collect()),
        Value::Object(o) => CstInputValue::Object(o.iter().map(|(k, v)| (k.clone(), to_input(v))).collect()),
    }
}

/// Parses `text`, lets `edit` change the top-level object, and prints the tree back.
fn json_edit(text: &str, edit: impl FnOnce(&CstObject) -> Res<()>) -> Res<String> {
    let (bom, body) = split_bom(text);
    let fresh = body.trim().is_empty();
    let root = CstRootNode::parse(if fresh { "{}" } else { body }, &ParseOptions::default()).map_err(|e| format!("not valid JSON: {e}"))?;
    let obj = root.object_value().ok_or("the top level of the file is not an object")?;
    edit(&obj)?;
    let mut out = root.to_string();
    if fresh && !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(format!("{bom}{out}"))
}

/// The object at `path` below `obj`, created when missing; a value of another type is an error.
fn object_at(obj: &CstObject, path: &[&str]) -> Res<CstObject> {
    let mut cur = obj.clone();
    for key in path {
        cur = cur.object_value_or_create(key).ok_or_else(|| format!("\"{key}\" is not an object"))?;
    }
    Ok(cur)
}

/// Sets `<path>.<key>` to `value`, replacing what was there.
pub fn json_set(text: &str, path: &[&str], key: &str, value: &Value) -> Res<String> {
    json_edit(text, |root| {
        let parent = object_at(root, path)?;
        match parent.get(key) {
            Some(prop) => prop.set_value(to_input(value)),
            None => {
                parent.append(key, to_input(value));
            }
        }
        Ok(())
    })
}

/// Removes `<path>.<key>`; nothing happens when it is not there.
pub fn json_remove(text: &str, path: &[&str], key: &str) -> Res<String> {
    json_edit(text, |root| {
        let mut cur = root.clone();
        for step in path {
            match cur.object_value(step) {
                Some(o) => cur = o,
                None => return Ok(()),
            }
        }
        if let Some(prop) = cur.get(key) {
            prop.remove();
        }
        Ok(())
    })
}

/// Adds or removes `member` in the string array `<path>.<key>`.
pub fn json_array_member(text: &str, path: &[&str], key: &str, member: &str, present: bool) -> Res<String> {
    let current = json_read(text)?;
    let mut node = &current;
    for step in path {
        node = node.get(*step).unwrap_or(&Value::Null);
    }
    let mut items: Vec<String> = node.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let had = items.iter().any(|m| m == member);
    if present == had {
        return Ok(text.to_string());
    }
    if present {
        items.push(member.to_string());
    } else {
        items.retain(|m| m != member);
    }
    json_set(text, path, key, &Value::Array(items.into_iter().map(Value::String).collect()))
}

/// The value at `path` of a document, if any.
pub fn json_at<'a>(doc: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(doc, |v, k| v.get(*k))
}

// ---------- TOML ----------

pub fn toml_parse(text: &str) -> Res<DocumentMut> {
    DocumentMut::from_str(split_bom(text).1).map_err(|e| format!("not valid TOML: {}", e.message()))
}

pub fn toml_check(text: &str) -> Res<()> {
    toml_parse(text).map(|_| ())
}

fn item_to_json(item: &Item) -> Value {
    use toml_edit::Value as T;
    fn value(v: &T) -> Value {
        match v {
            T::String(s) => Value::String(s.value().clone()),
            T::Integer(i) => Value::from(*i.value()),
            T::Float(f) => serde_json::Number::from_f64(*f.value()).map_or(Value::Null, Value::Number),
            T::Boolean(b) => Value::Bool(*b.value()),
            T::Datetime(d) => Value::String(d.value().to_string()),
            T::Array(a) => Value::Array(a.iter().map(value).collect()),
            T::InlineTable(t) => Value::Object(t.iter().map(|(k, v)| (k.to_string(), value(v))).collect()),
        }
    }
    match item {
        Item::Value(v) => value(v),
        Item::Table(t) => Value::Object(t.iter().map(|(k, v)| (k.to_string(), item_to_json(v))).collect()),
        Item::ArrayOfTables(a) => Value::Array(a.iter().map(|t| item_to_json(&Item::Table(t.clone()))).collect()),
        Item::None => Value::Null,
    }
}

fn json_to_item(v: &Value) -> Option<Item> {
    Some(match v {
        Value::Null => return None,
        Value::Bool(b) => toml_edit::value(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => toml_edit::value(i),
            None => toml_edit::value(n.as_f64()?),
        },
        Value::String(s) => toml_edit::value(s.as_str()),
        Value::Array(a) => {
            let mut arr = toml_edit::Array::new();
            for x in a {
                match json_to_item(x)? {
                    Item::Value(v) => arr.push(v),
                    _ => return None,
                }
            }
            Item::Value(arr.into())
        }
        Value::Object(o) => {
            let mut t = Table::new();
            for (k, x) in o {
                if let Some(item) = json_to_item(x) {
                    t.insert(k, item);
                }
            }
            Item::Table(t)
        }
    })
}

/// The whole document as a value (for reading, never for writing back).
#[cfg(test)]
pub fn toml_value(text: &str) -> Res<Value> {
    Ok(item_to_json(toml_parse(text)?.as_item()))
}

/// The `[mcp_servers.*]` tables as values, in file order.
pub fn toml_servers(text: &str) -> Res<Vec<(String, Value)>> {
    let doc = toml_parse(text)?;
    Ok(match doc.get("mcp_servers") {
        Some(Item::Table(t)) => t.iter().map(|(k, v)| (k.to_string(), item_to_json(v))).collect(),
        Some(Item::Value(toml_edit::Value::InlineTable(t))) => t.iter().map(|(k, v)| (k.to_string(), item_to_json(&Item::Value(v.clone())))).collect(),
        _ => Vec::new(),
    })
}

fn servers_table(doc: &mut DocumentMut) -> Res<&mut Table> {
    if !doc.contains_key("mcp_servers") {
        let mut t = Table::new();
        // Only the server tables get headers, not an empty `[mcp_servers]` above them.
        t.set_implicit(true);
        doc.insert("mcp_servers", Item::Table(t));
    }
    doc["mcp_servers"].as_table_mut().ok_or_else(|| "\"mcp_servers\" is not a table".to_string())
}

/// Sets `[mcp_servers.<name>]` from `server` (an object), keeping the comments above the old one.
pub fn toml_set_server(text: &str, name: &str, server: &Value) -> Res<String> {
    let mut doc = toml_parse(text)?;
    let Some(Item::Table(mut table)) = json_to_item(server) else { return Err("a server is a table".into()) };
    let servers = servers_table(&mut doc)?;
    if let Some(Item::Table(old)) = servers.get(name) {
        *table.decor_mut() = old.decor().clone();
    }
    servers.insert(name, Item::Table(table));
    Ok(with_bom(text, doc.to_string()))
}

pub fn toml_remove_server(text: &str, name: &str) -> Res<String> {
    let mut doc = toml_parse(text)?;
    if let Some(Item::Table(t)) = doc.get_mut("mcp_servers") {
        t.remove(name);
    }
    Ok(with_bom(text, doc.to_string()))
}

/// Sets one key of `[mcp_servers.<name>]`.
pub fn toml_set_server_key(text: &str, name: &str, key: &str, value: &Value) -> Res<String> {
    let mut doc = toml_parse(text)?;
    let Some(Item::Table(server)) = servers_table(&mut doc)?.get_mut(name) else { return Err(format!("no server named {name}")) };
    if let Some(item) = json_to_item(value) {
        server.insert(key, item);
    }
    Ok(with_bom(text, doc.to_string()))
}

/// Sets a top-level string key, in place when it is already there (not the same key inside a
/// table), else before the first table. Shared with the account code, which keeps a Codex login in
/// a file.
pub fn toml_set_top_level(text: &str, key: &str, value: &str) -> Res<String> {
    let mut doc = toml_parse(text)?;
    let mut new = toml_edit::Value::from(value);
    if let Some(Item::Value(old)) = doc.get(key) {
        // A comment after the old value stays.
        *new.decor_mut() = old.decor().clone();
    }
    doc.insert(key, Item::Value(new));
    Ok(with_bom(text, doc.to_string()))
}

/// `out` with the file's byte order mark and line endings, which `toml_edit` does not keep.
fn with_bom(original: &str, out: String) -> String {
    let out = restore_newlines(original, out);
    if original.starts_with(BOM) { format!("{BOM}{out}") } else { out }
}

/// `out` with the original's CRLF line endings back. A line that was in the original gets the ending
/// it had; a new line gets the file's usual one. Files that mix both keep each line as it was.
fn restore_newlines(original: &str, out: String) -> String {
    if !original.contains("\r\n") || out.contains('\r') {
        return out;
    }
    let mut endings: std::collections::HashMap<&str, Vec<bool>> = std::collections::HashMap::new();
    let (mut crlf_lines, mut lf_lines) = (0, 0);
    for line in original.split_inclusive('\n') {
        let crlf = line.ends_with("\r\n");
        if line.ends_with('\n') {
            if crlf { crlf_lines += 1 } else { lf_lines += 1 }
        }
        endings.entry(line.trim_end_matches(['\r', '\n'])).or_default().push(crlf);
    }
    let usual = crlf_lines >= lf_lines;
    let mut res = String::with_capacity(out.len() + out.len() / 16);
    for line in out.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        let crlf = endings.get_mut(body).filter(|v| !v.is_empty()).map(|v| v.remove(0)).unwrap_or(usual);
        res.push_str(body);
        if line.ends_with('\n') {
            res.push_str(if crlf { "\r\n" } else { "\n" });
        }
    }
    res
}

// ---------- masking by position ----------

const SECRET_TABLES: &[&str] = &["env", "environment", "headers", "http_headers"];
const SECRET_WORDS: &[&str] = &["token", "secret", "password", "passwd", "apikey", "api_key", "api-key", "auth", "bearer", "credential", "private_key", "private-key", "privatekey", "access_key", "access-key"];

/// A key whose string value is a credential: in an environment or header table, or named like one
/// (variable names such as `bearer_token_env_var` hold no secret).
fn secret_key(name: &str, in_table: bool) -> bool {
    let l = name.to_ascii_lowercase();
    if l.ends_with("_env_var") || l == "env_vars" {
        return false;
    }
    in_table || SECRET_WORDS.iter().any(|w| l.contains(w))
}

fn is_reference(value: &str) -> bool {
    value.contains("${") || value.contains("{env:") || (value.starts_with('$') && value.len() > 1)
}

fn mask_json_object(obj: &CstObject, in_table: bool, mask: &str) {
    for prop in obj.properties() {
        let (Some(name), Some(value)) = (prop.decoded_name(), prop.value()) else { continue };
        if let Some(text) = value.as_string_lit() {
            let literal = text.decoded_value().unwrap_or_default();
            if secret_key(&name, in_table) && !literal.is_empty() && !is_reference(&literal) {
                text.set_raw_value(format!("\"{mask}\""));
            }
        } else if let Some(inner) = value.as_object() {
            mask_json_object(&inner, in_table || SECRET_TABLES.contains(&name.as_str()), mask);
        } else if let Some(list) = value.as_array() {
            for el in list.elements() {
                if let CstNode::Container(_) = &el {
                    if let Some(o) = el.as_object() {
                        mask_json_object(&o, false, mask);
                    }
                }
            }
        }
    }
}

/// The document with every credential value replaced by `mask`, found by where it sits (an
/// environment or header table, a key named like a secret) in the parsed tree, so how the value
/// was escaped in the file does not matter. None when it does not parse.
pub fn json_mask(text: &str, mask: &str) -> Option<String> {
    let (bom, body) = split_bom(text);
    let root = CstRootNode::parse(body, &ParseOptions::default()).ok()?;
    if let Some(obj) = root.object_value() {
        mask_json_object(&obj, false, mask);
    }
    Some(format!("{bom}{root}"))
}

fn mask_toml_value(name: &str, v: &mut toml_edit::Value, in_table: bool, mask: &str) {
    use toml_edit::Value as T;
    let replace = match v {
        T::String(s) => secret_key(name, in_table) && !s.value().is_empty() && !is_reference(s.value()),
        T::InlineTable(t) => {
            let inside = in_table || SECRET_TABLES.contains(&name);
            for (k, x) in t.iter_mut() {
                mask_toml_value(k.get(), x, inside, mask);
            }
            false
        }
        _ => false,
    };
    if replace {
        let decor = v.decor().clone();
        *v = T::from(mask);
        *v.decor_mut() = decor;
    }
}

fn mask_toml_table(t: &mut Table, in_table: bool, mask: &str) {
    for (k, item) in t.iter_mut() {
        let name = k.get().to_string();
        match item {
            Item::Value(v) => mask_toml_value(&name, v, in_table, mask),
            Item::Table(sub) => mask_toml_table(sub, in_table || SECRET_TABLES.contains(&name.as_str()), mask),
            Item::ArrayOfTables(list) => list.iter_mut().for_each(|sub| mask_toml_table(sub, false, mask)),
            Item::None => {}
        }
    }
}

/// [`json_mask`] for TOML.
pub fn toml_mask(text: &str, mask: &str) -> Option<String> {
    let mut doc = toml_parse(text).ok()?;
    mask_toml_table(doc.as_table_mut(), false, mask);
    Some(with_bom(text, doc.to_string()))
}
