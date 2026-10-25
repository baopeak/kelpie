//! One neutral description of an MCP server, and how each agent writes it down. Reading maps an
//! agent's entry to [`McpServer`]; writing maps it back for another agent's format. Environment
//! values and header values are secrets: they have no `Debug` output, are never serialised to the
//! UI, and [`for_destination`] decides what may be written where.

use std::fmt;

use serde_json::{json, Map, Value};

use super::Scope;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transport {
    Stdio,
    Http,
    Sse,
    /// An entry in a shape this agent's reader does not know: listed, never copied.
    Unknown,
}

impl Transport {
    pub fn label(self) -> &'static str {
        match self {
            Transport::Stdio => "stdio",
            Transport::Http => "http",
            Transport::Sse => "sse",
            Transport::Unknown => "unknown",
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct McpServer {
    pub name: String,
    pub transport: Transport,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// Working folder of a program server (Gemini and Codex have the setting).
    pub cwd: String,
    pub enabled: bool,
    /// Keys of the source entry this model has no place for (timeouts, working folder…): a copy
    /// does not carry them, and says so.
    pub extra: Vec<String>,
}

/// Keys and counts only: a server's secrets never reach a log line or an assertion message.
impl fmt::Debug for McpServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpServer")
            .field("name", &self.name)
            .field("transport", &self.transport)
            .field("command", &self.command)
            .field("args", &self.args.len())
            .field("env", &self.env.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>())
            .field("url", &self.url)
            .field("headers", &self.headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>())
            .field("enabled", &self.enabled)
            .finish()
    }
}

impl McpServer {
    pub fn new(name: &str, transport: Transport) -> McpServer {
        McpServer { name: name.into(), transport, command: String::new(), args: Vec::new(), env: Vec::new(), url: String::new(), headers: Vec::new(), cwd: String::new(), enabled: true, extra: Vec::new() }
    }
}

/// How an agent spells its config.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Claude,
    Cursor,
    Gemini,
    OpenCode,
    Codex,
}

impl Kind {
    pub fn of(agent: &str) -> Option<Kind> {
        Some(match agent {
            "claude" => Kind::Claude,
            "cursor" => Kind::Cursor,
            "gemini" => Kind::Gemini,
            "opencode" => Kind::OpenCode,
            "codex" => Kind::Codex,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Claude => "Claude Code",
            Kind::Cursor => "Cursor",
            Kind::Gemini => "Gemini",
            Kind::OpenCode => "OpenCode",
            Kind::Codex => "Codex",
        }
    }

    /// The table the servers live in, below the document root.
    pub fn servers_key(self) -> &'static str {
        match self {
            Kind::OpenCode => "mcp",
            Kind::Codex => "mcp_servers",
            _ => "mcpServers",
        }
    }
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn list(v: &Value, key: &str) -> Vec<String> {
    v.get(key).and_then(Value::as_array).map(|a| a.iter().map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_string)).collect()).unwrap_or_default()
}

fn pairs(v: &Value, key: &str) -> Vec<(String, String)> {
    v.get(key).and_then(Value::as_object).map(|o| o.iter().map(|(k, x)| (k.clone(), x.as_str().map_or_else(|| x.to_string(), str::to_string))).collect()).unwrap_or_default()
}

fn extras(v: &Value, known: &[&str]) -> Vec<String> {
    v.as_object().map(|o| o.keys().filter(|k| !known.contains(&k.as_str())).cloned().collect()).unwrap_or_default()
}

/// An agent's entry as a neutral server.
pub fn decode(kind: Kind, name: &str, v: &Value) -> McpServer {
    let mut s = McpServer::new(name, Transport::Unknown);
    match kind {
        Kind::Claude | Kind::Cursor => {
            if let Some(c) = text(v, "command") {
                s.transport = Transport::Stdio;
                s.command = c;
            } else if let Some(u) = text(v, "url") {
                s.transport = if text(v, "type").as_deref() == Some("sse") { Transport::Sse } else { Transport::Http };
                s.url = u;
            }
            s.extra = extras(v, &["type", "command", "args", "env", "url", "headers"]);
        }
        Kind::Gemini => {
            if let Some(c) = text(v, "command") {
                s.transport = Transport::Stdio;
                s.command = c;
            } else if let Some(u) = text(v, "httpUrl") {
                s.transport = Transport::Http;
                s.url = u;
            } else if let Some(u) = text(v, "url") {
                s.transport = Transport::Sse;
                s.url = u;
            }
            s.cwd = text(v, "cwd").unwrap_or_default();
            s.extra = extras(v, &["command", "args", "env", "url", "httpUrl", "headers", "type", "cwd"]);
        }
        Kind::OpenCode => {
            match text(v, "type").as_deref() {
                Some("local") => {
                    let mut cmd = list(v, "command").into_iter();
                    s.transport = Transport::Stdio;
                    s.command = cmd.next().unwrap_or_default();
                    s.args = cmd.collect();
                }
                Some("remote") => {
                    s.transport = Transport::Http;
                    s.url = text(v, "url").unwrap_or_default();
                }
                _ => {}
            }
            s.enabled = v.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            s.extra = extras(v, &["type", "command", "environment", "url", "headers", "enabled"]);
        }
        Kind::Codex => {
            if let Some(c) = text(v, "command") {
                s.transport = Transport::Stdio;
                s.command = c;
            } else if let Some(u) = text(v, "url") {
                s.transport = Transport::Http;
                s.url = u;
            }
            s.enabled = v.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            s.cwd = text(v, "cwd").unwrap_or_default();
            s.extra = extras(v, &["command", "args", "env", "url", "http_headers", "enabled", "cwd", "env_vars", "env_http_headers", "bearer_token_env_var"]);
        }
    }
    if kind != Kind::OpenCode {
        s.args = list(v, "args");
    }
    s.env = pairs(v, if kind == Kind::OpenCode { "environment" } else { "env" });
    s.headers = pairs(v, if kind == Kind::Codex { "http_headers" } else { "headers" });
    if kind == Kind::Codex {
        // Variables Codex reads from the user's environment, as references.
        s.env.extend(list(v, "env_vars").into_iter().map(|n| (n.clone(), format!("${{{n}}}"))));
        s.headers.extend(pairs(v, "env_http_headers").into_iter().map(|(h, var)| (h, format!("${{{var}}}"))));
        if let Some(var) = text(v, "bearer_token_env_var") {
            s.headers.push(("Authorization".into(), format!("Bearer ${{{var}}}")));
        }
    }
    s
}

/// A neutral server as an agent's entry; an error says why this agent cannot hold it.
pub fn encode(kind: Kind, s: &McpServer) -> Result<Value, String> {
    let mut o = Map::new();
    let env_key = if kind == Kind::OpenCode { "environment" } else { "env" };
    let header_key = if kind == Kind::Codex { "http_headers" } else { "headers" };
    let obj = |p: &[(String, String)]| Value::Object(p.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect());
    match s.transport {
        Transport::Stdio => {
            if s.command.trim().is_empty() {
                return Err("the server has no command".into());
            }
            if kind == Kind::OpenCode {
                o.insert("type".into(), json!("local"));
                o.insert("command".into(), Value::Array(std::iter::once(&s.command).chain(&s.args).map(|a| Value::String(a.clone())).collect()));
            } else {
                o.insert("command".into(), json!(s.command));
                if !s.args.is_empty() {
                    o.insert("args".into(), json!(s.args));
                }
            }
            if !s.cwd.is_empty() {
                if !matches!(kind, Kind::Gemini | Kind::Codex) {
                    return Err(format!("{} cannot set the working folder this server needs ({})", kind.label(), s.cwd));
                }
                o.insert("cwd".into(), json!(s.cwd));
            }
            if kind == Kind::Codex {
                // A variable under its own name is forwarded (`env_vars`); the rest are literal values.
                let (forwarded, literal): (Vec<_>, Vec<_>) = s.env.iter().partition(|(k, v)| reference_of(v) == Some(k.as_str()));
                if !literal.is_empty() {
                    if let Some((k, _)) = literal.iter().find(|(_, v)| has_reference(v)) {
                        return Err(format!("Codex can only pass {k} on under its own name"));
                    }
                    o.insert("env".into(), obj(&literal.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>()));
                }
                if !forwarded.is_empty() {
                    o.insert("env_vars".into(), json!(forwarded.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()));
                }
            } else if !s.env.is_empty() {
                o.insert(env_key.into(), obj(&s.env));
            }
        }
        Transport::Http | Transport::Sse => {
            if s.url.trim().is_empty() {
                return Err("the server has no URL".into());
            }
            let sse = s.transport == Transport::Sse;
            match kind {
                Kind::Claude => {
                    o.insert("type".into(), json!(if sse { "sse" } else { "http" }));
                    o.insert("url".into(), json!(s.url));
                }
                Kind::Cursor => {
                    o.insert("url".into(), json!(s.url));
                }
                Kind::Gemini => {
                    o.insert(if sse { "url" } else { "httpUrl" }.into(), json!(s.url));
                }
                Kind::OpenCode => {
                    o.insert("type".into(), json!("remote"));
                    o.insert("url".into(), json!(s.url));
                }
                Kind::Codex => {
                    if sse {
                        return Err("Codex cannot use a server that speaks SSE".into());
                    }
                    o.insert("url".into(), json!(s.url));
                }
            }
            if kind == Kind::Codex {
                let (mut literal, mut by_var, mut bearer) = (Vec::new(), Vec::new(), None);
                for (h, v) in &s.headers {
                    if let Some(var) = reference_of(v) {
                        by_var.push((h.clone(), var.to_string()));
                    } else if let Some(var) = h.eq_ignore_ascii_case("authorization").then(|| v.strip_prefix("Bearer ").and_then(reference_of)).flatten() {
                        bearer = Some(var.to_string());
                    } else if has_reference(v) {
                        return Err(format!("Codex cannot build the {h} header from variables"));
                    } else {
                        literal.push((h.clone(), v.clone()));
                    }
                }
                if !literal.is_empty() {
                    o.insert("http_headers".into(), obj(&literal));
                }
                if !by_var.is_empty() {
                    o.insert("env_http_headers".into(), Value::Object(by_var.into_iter().map(|(h, v)| (h, Value::String(v))).collect()));
                }
                if let Some(var) = bearer {
                    o.insert("bearer_token_env_var".into(), json!(var));
                }
            } else if !s.headers.is_empty() {
                o.insert(header_key.into(), obj(&s.headers));
            }
        }
        Transport::Unknown => return Err(format!("{} cannot read this entry's format", kind.label())),
    }
    Ok(Value::Object(o))
}

/// The variable a whole value refers to (`${X}`, `$X`, `${env:X}`, `{env:X}`), if it is only that.
pub fn reference_of(value: &str) -> Option<&str> {
    let v = value.trim();
    let name = v
        .strip_prefix("${env:")
        .or_else(|| v.strip_prefix("{env:"))
        .or_else(|| v.strip_prefix("${"))
        .and_then(|r| r.strip_suffix('}'))
        .or_else(|| v.strip_prefix('$').filter(|r| !r.starts_with('{')))?;
    is_name(name).then_some(name)
}

fn is_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The variables a value reads from the environment, in order.
pub fn reference_vars(value: &str) -> Vec<String> {
    if let Some(v) = reference_of(value) {
        return vec![v.to_string()];
    }
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(i) = rest.find(['$', '{']) {
        let r = &rest[i..];
        let (inner, advance) = if let Some(x) = r.strip_prefix("${") {
            match x.find('}') {
                Some(e) => (&x[..e], 2 + e + 1),
                None => break,
            }
        } else if let Some(x) = r.strip_prefix("{env:") {
            match x.find('}') {
                Some(e) => (&x[..e], 5 + e + 1),
                None => break,
            }
        } else {
            rest = &r[1..];
            continue;
        };
        out.push(inner.strip_prefix("env:").unwrap_or(inner).to_string());
        rest = &r[advance..];
    }
    out
}

/// Whether a value is (or contains) a reference to the environment rather than only a literal.
pub fn has_reference(value: &str) -> bool {
    reference_of(value).is_some() || value.contains("${") || value.contains("{env:")
}

/// How `kind` refers to an environment variable. Codex has no inline syntax: a variable under its
/// own name is forwarded (`env_vars`), written here as `${NAME}` and turned into that on encoding.
fn reference_for(kind: Kind, var: &str) -> String {
    match kind {
        Kind::Claude | Kind::Gemini | Kind::Codex => format!("${{{var}}}"),
        Kind::Cursor => format!("${{env:{var}}}"),
        Kind::OpenCode => format!("{{env:{var}}}"),
    }
}

/// `value` with every reference rewritten into `kind`'s syntax; none when one cannot be (a default
/// value such as `${X:-y}`, or any reference at all for Codex, which only forwards whole variables).
fn convert_refs(value: &str, kind: Kind) -> Option<String> {
    let mut out = String::new();
    let mut rest = value;
    while !rest.is_empty() {
        let (skip, inner) = if let Some(r) = rest.strip_prefix("${") {
            let end = r.find('}')?;
            (2 + end + 1, &r[..end])
        } else if let Some(r) = rest.strip_prefix("{env:") {
            let end = r.find('}')?;
            (5 + end + 1, &r[..end])
        } else {
            let c = rest.chars().next()?;
            out.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        };
        let name = inner.strip_prefix("env:").unwrap_or(inner);
        if kind == Kind::Codex || !is_name(name) {
            return None;
        }
        out.push_str(&reference_for(kind, name));
        rest = &rest[skip..];
    }
    Some(out)
}

/// A value in `kind`'s syntax, or none when it cannot be said there.
fn convert_value(value: &str, kind: Kind) -> Option<String> {
    match reference_of(value) {
        Some(var) => Some(reference_for(kind, var)),
        None => convert_refs(value, kind),
    }
}

/// One thing to tell the user about a copy, as a code the page translates and its parameters.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct Warn {
    pub code: &'static str,
    pub params: std::collections::BTreeMap<&'static str, String>,
}

pub fn warn(code: &'static str, params: &[(&'static str, String)]) -> Warn {
    Warn { code, params: params.iter().cloned().collect() }
}

/// What a copy of `src` looks like once it is written for `kind` in `scope`.
///
/// A server of the user's own: environment references are rewritten into the destination's syntax;
/// literal values become a reference to the variable of the same name unless the user opted in and
/// the file is their own; header values that are literals are left out.
///
/// A server that came from a project (`untrusted`) never gets references to the user's variables
/// made up for it, and carries nothing of its environment or headers unless the user opted in and
/// the destination is the user's own file, where the confirmation listed every value.
pub fn for_destination(src: &McpServer, kind: Kind, scope: Scope, include_env: bool, untrusted: bool) -> (McpServer, Vec<Warn>) {
    let mut s = src.clone();
    s.enabled = true;
    let mut notes = Vec::new();
    let carry = scope == Scope::User && include_env;
    let (mut referenced, mut left_out, mut unsaid, mut not_carried) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut env = Vec::new();
    // Between project files a reference alone (`${TOKEN}`) stays: it holds no value of the user's.
    let ref_only = |v: &str| untrusted && scope == Scope::Project && reference_of(v).is_some();
    for (k, v) in &src.env {
        if untrusted && !carry && !ref_only(v) {
            not_carried.push(k.clone());
        } else if has_reference(v) {
            let ok = if kind == Kind::Codex { reference_of(v) == Some(k.as_str()) } else { true };
            match convert_value(v, kind).filter(|_| ok) {
                Some(n) => env.push((k.clone(), n)),
                None if kind == Kind::Codex => left_out.push(k.clone()),
                None => {
                    unsaid.push(k.clone());
                    env.push((k.clone(), v.clone()));
                }
            }
        } else if carry {
            env.push((k.clone(), v.clone()));
        } else {
            // The user's own literal, not copied: the destination reads the variable of that name.
            referenced.push(k.clone());
            env.push((k.clone(), reference_for(kind, k)));
        }
    }
    s.env = env;
    let mut headers = Vec::new();
    let mut headers_out = Vec::new();
    let mut literal_out: Vec<String> = Vec::new();
    for (h, v) in &src.headers {
        if untrusted && !carry && !ref_only(v) {
            not_carried.push(h.clone());
        } else if has_reference(v) {
            let codex_ok = reference_of(v).is_some() || (h.eq_ignore_ascii_case("authorization") && v.strip_prefix("Bearer ").and_then(reference_of).is_some());
            let converted = if kind == Kind::Codex {
                codex_ok.then(|| match v.strip_prefix("Bearer ").and_then(reference_of) {
                    Some(var) => format!("Bearer {}", reference_for(kind, var)),
                    None => reference_for(kind, reference_of(v).unwrap_or_default()),
                })
            } else if let Some(var) = v.strip_prefix("Bearer ").and_then(reference_of) {
                Some(format!("Bearer {}", reference_for(kind, var)))
            } else {
                convert_value(v, kind)
            };
            match converted {
                Some(n) => headers.push((h.clone(), n)),
                None if kind == Kind::Codex => headers_out.push(h.clone()),
                None => {
                    unsaid.push(h.clone());
                    headers.push((h.clone(), v.clone()));
                }
            }
        } else if carry {
            headers.push((h.clone(), v.clone()));
        } else {
            // The user's own literal header (a token): not copied without the opt-in.
            literal_out.push(h.clone());
        }
    }
    s.headers = headers;
    if kind != Kind::Codex {
        for a in s.args.iter_mut().chain(std::iter::once(&mut s.url)) {
            if has_reference(a) {
                match convert_value(a, kind) {
                    Some(n) => *a = n,
                    None => unsaid.push(a.clone()),
                }
            }
        }
    } else if s.args.iter().chain([&s.url]).any(|a| has_reference(a)) {
        unsaid.push("args".into());
    }
    if !referenced.is_empty() {
        notes.push(warn("env_referenced", &[("keys", referenced.join(", "))]));
    }
    if !left_out.is_empty() || !headers_out.is_empty() {
        let keys: Vec<String> = left_out.into_iter().chain(headers_out).collect();
        notes.push(warn("env_unexpressible", &[("keys", keys.join(", ")), ("agent", kind.label().into())]));
    }
    if !unsaid.is_empty() {
        notes.push(warn("reference_kept", &[("keys", unsaid.join(", ")), ("agent", kind.label().into())]));
    }
    if !literal_out.is_empty() {
        notes.push(warn("headers_not_copied", &[("keys", literal_out.join(", "))]));
    }
    if !not_carried.is_empty() {
        notes.push(warn("env_not_carried", &[("keys", not_carried.join(", "))]));
    }
    if !src.extra.is_empty() {
        notes.push(warn("not_carried", &[("keys", src.extra.join(", "))]));
    }
    (s, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_are_recognised_in_every_spelling() {
        for (v, want) in [("${TOKEN}", Some("TOKEN")), ("$TOKEN", Some("TOKEN")), ("${env:TOKEN}", Some("TOKEN")), ("{env:TOKEN}", Some("TOKEN")), ("abc", None), ("${A:-x}", None), ("$", None), ("${}", None), ("Bearer ${T}", None)] {
            assert_eq!(reference_of(v), want, "{v}");
        }
    }
}
