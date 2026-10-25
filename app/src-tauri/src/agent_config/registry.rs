//! "Find more agents": the Agent Client Protocol registry, fetched once when the user asks. Each
//! entry becomes the full command Kelpie would save as a custom agent (version pinned), shown
//! before anything is stored. Nothing here runs a registry program.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::fs_ops::CmdResult;

const REGISTRY_URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
const MAX_BYTES: u64 = 4 << 20;
const MAX_AGENTS: usize = 500;

#[derive(Serialize, Debug, PartialEq)]
pub struct RegistryAgent {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub repository: Option<String>,
    /// How the registry ships it: npx | uvx | binary.
    pub distribution: String,
    /// The full command, program first, version pinned.
    pub command: Vec<String>,
    /// Environment variables the entry asks for (names only).
    pub env_keys: Vec<String>,
    /// The exact version the command runs (the registry's, unless the package named one).
    pub pinned: Option<String>,
    /// The agent as `agent.set_custom` takes it; none when Kelpie cannot run this entry.
    pub def: Option<Value>,
    /// Why this entry cannot be saved, as a code the page translates.
    pub problem: Option<String>,
}

/// The registry address. Only an end-to-end run may point it elsewhere (a local fake server).
pub fn registry_url() -> (String, bool) {
    if std::env::var_os("KELPIE_E2E").is_some() {
        if let Some(url) = std::env::var("KELPIE_ACP_REGISTRY_URL").ok().filter(|u| loopback_http(u)) {
            return (url, true);
        }
    }
    (REGISTRY_URL.into(), false)
}

/// A plain-http address on this computer: the only kind a test may point the registry at.
fn loopback_http(url: &str) -> bool {
    url.parse::<ureq::http::Uri>().ok().is_some_and(|u| u.scheme_str() == Some("http") && matches!(u.host(), Some("127.0.0.1" | "localhost" | "[::1]")))
}

/// An exact version: dotted numbers, with an optional pre-release or build tail.
fn exact_version(v: &str, min_parts: usize) -> bool {
    let core = v.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() >= min_parts && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) && v.chars().all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
}

fn npm_name(n: &str) -> bool {
    let ok = |p: &str| !p.is_empty() && !p.starts_with(['.', '_', '-']) && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c));
    match n.strip_prefix('@') {
        Some(rest) => rest.split_once('/').is_some_and(|(scope, name)| ok(scope) && ok(name)),
        None => ok(n),
    }
}

fn python_name(n: &str) -> bool {
    n.starts_with(|c: char| c.is_ascii_alphanumeric()) && n.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

/// `package` as an exact `name@version`. Only plain registry names are accepted (no URLs, paths or
/// `github:` specs); an exact version named in the package stays, any tag (`latest`, `beta`), range
/// or other spec is replaced by the registry's own version, which must itself be exact.
fn pinned(package: &str, version: &str, python: bool) -> Result<String, &'static str> {
    let (name, spec) = if python {
        let end = package.find(|c: char| !(c.is_ascii_alphanumeric() || "._-".contains(c))).unwrap_or(package.len());
        (&package[..end], package[end..].trim_start_matches(['@', '=']))
    } else {
        match package.rfind('@') {
            Some(i) if i > 0 => (&package[..i], &package[i + 1..]),
            _ => (package, ""),
        }
    };
    if !(if python { python_name(name) } else { npm_name(name) }) {
        return Err("package_name");
    }
    // What follows a Python name must be a version or a specifier, not a URL or a path.
    if python && !package[name.len()..].is_empty() && !package[name.len()..].starts_with(['@', '=', '<', '>', '!', '~', '[', ' ', ';']) {
        return Err("package_name");
    }
    let rest_is_plain = !python || package[name.len()..].is_empty() || package[name.len()..].starts_with(['@', '=']);
    // npm versions are three numbers: `1` and `1.2` are ranges.
    let parts = if python { 1 } else { 3 };
    let chosen = if exact_version(spec, parts) && rest_is_plain { spec } else if exact_version(version, parts) { version } else { return Err("version_not_exact") };
    Ok(format!("{name}@{chosen}"))
}

fn plain(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.chars().any(|c| c.is_control())
}

fn agent_id(registry_id: &str) -> Option<String> {
    let id: String = registry_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' }).collect();
    let id = format!("acp-{id}");
    (id.len() <= 48 && registry_id.len() <= 40).then_some(id)
}

fn entry(a: &Value) -> Option<RegistryAgent> {
    let text = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let (rid, name, version) = (text("id"), text("name"), text("version"));
    if !plain(&rid, 40) || !plain(&name, 80) || !plain(&version, 40) {
        return None;
    }
    let dist = a.get("distribution")?;
    let env_keys: Vec<String> = ["npx", "uvx"].iter().filter_map(|k| dist.get(*k)?.get("env")?.as_object()).flat_map(|e| e.keys().cloned()).collect();
    let mut out = RegistryAgent {
        id: rid.clone(),
        name: name.clone(),
        version: version.clone(),
        description: text("description").chars().take(300).collect(),
        repository: a.get("repository").and_then(Value::as_str).filter(|r| r.starts_with("https://")).map(str::to_string),
        distribution: String::new(),
        command: Vec::new(),
        env_keys,
        pinned: None,
        def: None,
        problem: None,
    };
    let runner = ["npx", "uvx"].into_iter().find_map(|k| dist.get(k).map(|d| (k, d)));
    let Some((bin, d)) = runner else {
        out.distribution = "binary".into();
        out.problem = Some("downloaded_program".into());
        return Some(out);
    };
    out.distribution = bin.into();
    let package = d.get("package").and_then(Value::as_str).unwrap_or("");
    let args: Vec<String> = d.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if !plain(package, 200) || package.starts_with('-') || package.contains(char::is_whitespace) || args.len() > 20 || !args.iter().all(|a| plain(a, 200)) {
        out.problem = Some("command_form".into());
        return Some(out);
    }
    let Some(id) = agent_id(&rid) else {
        out.problem = Some("id_long".into());
        return Some(out);
    };
    let spec = match pinned(package, &version, bin == "uvx") {
        Ok(p) => p,
        Err(code) => {
            out.problem = Some(code.into());
            return Some(out);
        }
    };
    out.pinned = spec.rsplit_once('@').map(|(_, v)| v.to_string());
    let acp: Vec<String> = if bin == "npx" { ["-y".to_string(), spec].into_iter().chain(args).collect() } else { std::iter::once(spec).chain(args).collect() };
    out.command = std::iter::once(bin.to_string()).chain(acp.iter().cloned()).collect();
    out.def = Some(json!({ "id": id, "label": name, "bin": bin, "launch": [], "yolo": [], "resume_latest": [], "prompt": "typed", "acp": acp }));
    Some(out)
}

/// The entries of a registry document.
pub fn parse(body: &str) -> CmdResult<Vec<RegistryAgent>> {
    let doc: Value = serde_json::from_str(body).map_err(|_| "the registry answered with something unreadable".to_string())?;
    let agents = doc.get("agents").and_then(Value::as_array).ok_or("the registry has no agent list")?;
    Ok(agents.iter().take(MAX_AGENTS).filter_map(entry).collect())
}

/// One GET of the registry. https only, a few seconds, a few megabytes.
fn fetch(url: &str, allow_http: bool) -> CmdResult<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).https_only(!allow_http).max_redirects(3).http_status_as_error(false).build().into();
    let mut resp = agent.get(url).call().map_err(|e| format!("could not reach the registry: {}", e.to_string().chars().take(120).collect::<String>()))?;
    if resp.status().as_u16() != 200 {
        return Err(format!("the registry answered HTTP {}", resp.status().as_u16()));
    }
    resp.body_mut().with_config().limit(MAX_BYTES).read_to_string().map_err(|_| "the registry's answer was too large or unreadable".to_string())
}

pub fn fetch_agents(url: &str, allow_http: bool) -> CmdResult<Vec<RegistryAgent>> {
    parse(&fetch(url, allow_http)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_pinned_in_the_command() {
        let npm = |p: &str| pinned(p, "1.2.3", false);
        assert_eq!(npm("tool"), Ok("tool@1.2.3".into()));
        assert_eq!(npm("tool@latest"), Ok("tool@1.2.3".into()));
        assert_eq!(npm("tool@beta"), Ok("tool@1.2.3".into()));
        assert_eq!(npm("tool@^1.0.0"), Ok("tool@1.2.3".into()));
        assert_eq!(npm("tool@0.9.0"), Ok("tool@0.9.0".into()));
        assert_eq!(npm("tool@2.0.0-rc.1"), Ok("tool@2.0.0-rc.1".into()));
        assert_eq!(npm("tool@1"), Ok("tool@1.2.3".into()), "a bare major is a range");
        assert_eq!(npm("tool@1.2"), Ok("tool@1.2.3".into()));
        assert_eq!(pinned("tool", "1.2", false), Err("version_not_exact"));
        assert_eq!(npm("@scope/tool"), Ok("@scope/tool@1.2.3".into()));
        assert_eq!(npm("@scope/tool@next"), Ok("@scope/tool@1.2.3".into()));
        assert_eq!(npm("@scope/tool@2.0.0"), Ok("@scope/tool@2.0.0".into()));
        for bad in ["https://host/x.tgz", "github:user/repo", "file:../x", "./local", "../x", "a/b", "Tool Name", "-y", "tool@https://host/x.tgz@1.0"] {
            assert_eq!(npm(bad), Err("package_name"), "{bad}");
        }
        assert_eq!(pinned("tool", "latest", false), Err("version_not_exact"));
        assert_eq!(pinned("tool@^1", "main", false), Err("version_not_exact"));
        let uv = |p: &str| pinned(p, "0.4.1", true);
        assert_eq!(uv("pkg"), Ok("pkg@0.4.1".into()));
        assert_eq!(uv("pkg==0.3.0"), Ok("pkg@0.3.0".into()));
        assert_eq!(uv("pkg@0.3.0"), Ok("pkg@0.3.0".into()));
        assert_eq!(uv("pkg>=1"), Ok("pkg@0.4.1".into()));
        assert_eq!(uv("pkg[extra]~=1.0"), Ok("pkg@0.4.1".into()));
        assert_eq!(uv("pkg@latest"), Ok("pkg@0.4.1".into()));
        assert_eq!(uv("git+https://host/x"), Err("package_name"));
        assert_eq!(uv("https://host/x.whl"), Err("package_name"));
    }

    const REGISTRY: &str = r#"{"version":"1.0.0","agents":[
        {"id":"alpha-acp","name":"Alpha","version":"1.4.0","description":"d","repository":"https://example.com/a","distribution":{"npx":{"package":"@acme/alpha","args":["--acp"],"env":{"ALPHA_KEY":"x"}}}},
        {"id":"beta","name":"Beta","version":"0.2.0","distribution":{"uvx":{"package":"beta-agent","args":[]}}},
        {"id":"gamma","name":"Gamma","version":"3.0.0","distribution":{"binary":{"linux-x86_64":{"archive":"https://x/y.tgz","cmd":"./g"}}}},
        {"id":"evil","name":"Evil","version":"1.0.0","distribution":{"npx":{"package":"-y --yes","args":[]}}},
        {"id":"tagged","name":"Tagged","version":"2.5.0","distribution":{"npx":{"package":"tagged-agent@beta","args":[]}}},
        {"id":"url","name":"Url","version":"1.0.0","distribution":{"npx":{"package":"https://host/x.tgz","args":[]}}},
        {"id":"bad\u0000id","name":"Bad","version":"1","distribution":{"npx":{"package":"p"}}}
    ],"extensions":[]}"#;

    #[test]
    fn registry_entries_become_pinned_commands_and_unsafe_ones_are_refused() {
        let all = parse(REGISTRY).unwrap();
        assert_eq!(all.len(), 6, "the entry with a control character in its id is dropped");
        let tagged = all.iter().find(|a| a.id == "tagged").unwrap();
        assert_eq!((tagged.command.clone(), tagged.pinned.as_deref()), (vec!["npx".to_string(), "-y".into(), "tagged-agent@2.5.0".into()], Some("2.5.0")), "a tag is replaced by the registry's exact version");
        assert_eq!(all.iter().find(|a| a.id == "url").unwrap().problem.as_deref(), Some("package_name"));
        let alpha = &all[0];
        assert_eq!(alpha.command, ["npx", "-y", "@acme/alpha@1.4.0", "--acp"]);
        assert_eq!(alpha.env_keys, ["ALPHA_KEY"]);
        let def = alpha.def.as_ref().unwrap();
        assert_eq!((def["id"].as_str(), def["bin"].as_str()), (Some("acp-alpha-acp"), Some("npx")));
        assert_eq!(def["acp"], json!(["-y", "@acme/alpha@1.4.0", "--acp"]));
        assert_eq!(all[1].command, ["uvx", "beta-agent@0.2.0"]);
        assert!(all[2].def.is_none() && all[2].problem.as_deref() == Some("downloaded_program"));
        assert!(all[3].def.is_none() && all[3].problem.is_some(), "a package that looks like an option is not saved");
    }

    #[test]
    fn an_unreadable_registry_is_an_error() {
        assert!(parse("<html>").is_err());
        assert!(parse("{}").is_err());
    }

    /// A one-shot local server: answers the first request with `body`, returns the address and the
    /// number of requests it saw once joined.
    fn serve_once(body: String) -> (String, std::thread::JoinHandle<usize>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/registry.json", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let handle = std::thread::spawn(move || {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(4);
            let mut seen = 0;
            while std::time::Instant::now() < end {
                if let Ok((mut c, _)) = listener.accept() {
                    c.set_nonblocking(false).unwrap();
                    let mut buf = [0u8; 2048];
                    let _ = c.read(&mut buf);
                    seen += 1;
                    let _ = write!(c, "HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: {}
Connection: close

{body}", body.len());
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            seen
        });
        (url, handle)
    }

    #[test]
    fn the_registry_is_one_https_request_and_a_local_server_only_when_allowed() {
        let (url, server) = serve_once(REGISTRY.to_string());
        // Plain http is refused before anything is sent.
        assert!(fetch_agents(&url, false).is_err());
        let agents = fetch_agents(&url, true).unwrap();
        assert_eq!(agents.len(), 6);
        assert_eq!(server.join().unwrap(), 1, "one request in all");
        // The address can only be moved by an end-to-end run.
        assert!(std::env::var_os("KELPIE_E2E").is_none());
        assert_eq!(registry_url(), (REGISTRY_URL.to_string(), false));
        assert!(loopback_http("http://127.0.0.1:4000/registry.json") && loopback_http("http://localhost:1/x"));
        assert!(!loopback_http("http://example.com/registry.json") && !loopback_http("http://127.0.0.1.evil.example/") && !loopback_http("https://127.0.0.1:1/"));
        assert!(!loopback_http("http://127.0.0.1@evil.example:80/x") && !loopback_http("http://localhost.evil.example/") && loopback_http("http://127.0.0.1/registry.json"));
        // A reply past the size cap is an error, not memory.
        let (url, server) = serve_once("x".repeat(MAX_BYTES as usize + 10));
        assert!(fetch_agents(&url, true).is_err());
        let _ = server.join();
    }
}
