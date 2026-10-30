//! Issue trackers and GitLab: Linear (GraphQL, personal API key), Jira (Cloud REST v3 with
//! e-mail + API token, or Server / Data Center v2 with a personal access token) and GitLab
//! (REST v4 with a personal access token; public projects are readable without one).
//!
//! Tokens live in the OS credential store (Windows Credential Manager, macOS Keychain) under
//! a service named after the daemon endpoint, are sent only to the service they belong to and
//! never reach the webview, which only learns whether one is set.

use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};

use crate::fs_ops::{err, CmdResult};

// ---------- secrets ----------

fn valid_key(key: &str) -> CmdResult<()> {
    let ok = matches!(key.split(':').next(), Some("linear" | "jira" | "gitlab")) && key.len() < 300 && !key.contains(['\n', '\r']);
    if ok { Ok(()) } else { Err("unknown integration".into()) }
}

fn secret(key: &str) -> Option<String> {
    kelpie_proto::secret_get(key)
}

#[tauri::command]
pub async fn integration_secret_set(key: String, value: String) -> CmdResult<()> {
    crate::fs_ops::blocking(move || {
        valid_key(&key)?;
        let value = value.trim();
        if value.is_empty() {
            return Err("the token is empty".into());
        }
        kelpie_proto::secret_set(&key, value).map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn integration_secret_clear(key: String) -> CmdResult<()> {
    crate::fs_ops::blocking(move || {
        valid_key(&key)?;
        kelpie_proto::secret_delete(&key).map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn integration_secret_has(key: String) -> CmdResult<bool> {
    crate::fs_ops::blocking(move || {
        valid_key(&key)?;
        Ok(secret(&key).is_some())
    })
    .await
}

// ---------- HTTP ----------

const USER_AGENT: &str = concat!("kelpie/", env!("CARGO_PKG_VERSION"));

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).http_status_as_error(false).build().into()
}

/// Status and JSON body; error texts never include request headers (no token can leak).
fn send(req: Result<ureq::http::Response<ureq::Body>, ureq::Error>, what: &str) -> CmdResult<(u16, Value)> {
    let mut resp = req.map_err(|e| format!("{what}: network error ({})", e.to_string().chars().take(120).collect::<String>()))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok((status, body))
}

fn http_error(what: &str, status: u16, body: &Value) -> String {
    let detail = body["errorMessages"][0]
        .as_str()
        .or(body["message"].as_str())
        .or(body["errors"][0]["message"].as_str())
        .or(body["error"].as_str())
        .unwrap_or("");
    let hint = match status {
        401 => " (sign-in rejected: check the token)",
        403 => " (not allowed: check the token's scopes)",
        404 => " (not found)",
        429 => " (rate limited, try again later)",
        _ => "",
    };
    format!("{what}: HTTP {status}{hint}{}{}", if detail.is_empty() { "" } else { ": " }, detail.chars().take(200).collect::<String>())
}

/// One issue, the same shape for every tracker.
#[derive(Serialize, Debug, Default, PartialEq)]
pub struct Issue {
    source: &'static str,
    /// The tracker's id (Linear UUID, Jira key, GitLab issue iid).
    id: String,
    /// Human key: ENG-123, PROJ-7, #42.
    key: String,
    title: String,
    url: String,
    state: String,
    /// open | progress | done | other
    state_kind: String,
    priority: Option<String>,
    /// Suggested branch name (Linear).
    branch: Option<String>,
    description: Option<String>,
    team: Option<String>,
    updated: Option<String>,
}

const MAX_DESCRIPTION: usize = 6000;

fn clip(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| if s.chars().count() > MAX_DESCRIPTION { s.chars().take(MAX_DESCRIPTION).collect::<String>() + "…" } else { s.to_string() })
}

// ---------- Linear ----------

fn linear_url() -> String {
    // Tests point this at a local server.
    std::env::var("KELPIE_LINEAR_API").unwrap_or_else(|_| "https://api.linear.app/graphql".into())
}

fn linear(query: &str, variables: Value) -> CmdResult<Value> {
    let token = secret("linear").ok_or("Linear is not connected (Settings → Integrations)")?;
    let req = agent().post(&linear_url()).header("Authorization", &token).header("User-Agent", USER_AGENT).send_json(json!({"query": query, "variables": variables}));
    let (status, body) = send(req, "Linear")?;
    if let Some(e) = body["errors"].as_array().and_then(|a| a.first()) {
        let code = e["extensions"]["code"].as_str().unwrap_or("");
        let msg = e["message"].as_str().unwrap_or("error");
        return Err(if code == "RATELIMITED" { "Linear: rate limited, try again later".into() } else { format!("Linear: {msg}") });
    }
    if status != 200 {
        return Err(http_error("Linear", status, &body));
    }
    Ok(body["data"].clone())
}

const LINEAR_FIELDS: &str = "id identifier title url branchName description priorityLabel updatedAt state { name type } team { key }";

fn linear_kind(t: &str) -> &'static str {
    match t {
        "started" => "progress",
        "completed" | "canceled" | "duplicate" => "done",
        "triage" | "backlog" | "unstarted" => "open",
        _ => "other",
    }
}

fn linear_issue(n: &Value) -> Issue {
    Issue {
        source: "linear",
        id: n["id"].as_str().unwrap_or_default().into(),
        key: n["identifier"].as_str().unwrap_or_default().into(),
        title: n["title"].as_str().unwrap_or_default().into(),
        url: n["url"].as_str().unwrap_or_default().into(),
        state: n["state"]["name"].as_str().unwrap_or_default().into(),
        state_kind: linear_kind(n["state"]["type"].as_str().unwrap_or_default()).into(),
        priority: n["priorityLabel"].as_str().filter(|p| *p != "No priority").map(str::to_string),
        branch: n["branchName"].as_str().filter(|b| !b.is_empty()).map(str::to_string),
        description: n["description"].as_str().and_then(clip),
        team: n["team"]["key"].as_str().map(str::to_string),
        updated: n["updatedAt"].as_str().map(str::to_string),
    }
}

fn linear_issues_blocking(search: Option<String>) -> CmdResult<Vec<Issue>> {
    let data = match search.filter(|s| !s.trim().is_empty()) {
        Some(q) => linear(&format!("query($q: String!) {{ searchIssues(term: $q, first: 30) {{ nodes {{ {LINEAR_FIELDS} }} }} }}"), json!({"q": q.trim()}))?,
        None => linear(
            &format!("query {{ viewer {{ assignedIssues(first: 50, orderBy: updatedAt, filter: {{ state: {{ type: {{ nin: [\"completed\", \"canceled\", \"duplicate\"] }} }} }}) {{ nodes {{ {LINEAR_FIELDS} }} }} }} }}"),
            json!({}),
        )?,
    };
    let nodes = data["searchIssues"]["nodes"].as_array().or(data["viewer"]["assignedIssues"]["nodes"].as_array()).cloned().unwrap_or_default();
    Ok(nodes.iter().map(linear_issue).collect())
}

#[tauri::command]
pub async fn linear_issues(search: Option<String>) -> CmdResult<Vec<Issue>> {
    crate::fs_ops::blocking(move || linear_issues_blocking(search)).await
}

#[derive(Serialize, Debug, PartialEq)]
pub struct IssueState {
    id: String,
    name: String,
    state_kind: String,
}

/// Workflow states of the issue's team (to move it).
#[tauri::command]
pub async fn linear_states(issue_id: String) -> CmdResult<Vec<IssueState>> {
    crate::fs_ops::blocking(move || {
        let data = linear("query($id: String!) { issue(id: $id) { team { states { nodes { id name type position } } } } }", json!({"id": issue_id}))?;
        let mut nodes = data["issue"]["team"]["states"]["nodes"].as_array().cloned().unwrap_or_default();
        nodes.sort_by(|a, b| a["position"].as_f64().unwrap_or(0.0).total_cmp(&b["position"].as_f64().unwrap_or(0.0)));
        Ok(nodes
            .iter()
            .map(|n| IssueState { id: n["id"].as_str().unwrap_or_default().into(), name: n["name"].as_str().unwrap_or_default().into(), state_kind: linear_kind(n["type"].as_str().unwrap_or_default()).into() })
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn linear_set_state(issue_id: String, state_id: String) -> CmdResult<()> {
    crate::fs_ops::blocking(move || {
        let data = linear("mutation($id: String!, $state: String!) { issueUpdate(id: $id, input: { stateId: $state }) { success } }", json!({"id": issue_id, "state": state_id}))?;
        if data["issueUpdate"]["success"].as_bool() == Some(true) { Ok(()) } else { Err("Linear did not update the issue".into()) }
    })
    .await
}

/// Who the key belongs to (to confirm a new key works).
#[tauri::command]
pub async fn linear_viewer() -> CmdResult<String> {
    crate::fs_ops::blocking(|| {
        let data = linear("query { viewer { name email } }", json!({}))?;
        Ok(format!("{} <{}>", data["viewer"]["name"].as_str().unwrap_or("?"), data["viewer"]["email"].as_str().unwrap_or("?")))
    })
    .await
}

// ---------- Jira ----------

/// A Jira site: Cloud (`email` set: Basic e-mail + API token, REST v3) or Server / Data
/// Center (no e-mail: Bearer personal access token, REST v2).
#[derive(serde::Deserialize, Clone)]
pub struct JiraSite {
    url: String,
    #[serde(default)]
    email: String,
}

impl JiraSite {
    fn base(&self) -> String {
        self.url.trim().trim_end_matches('/').to_string()
    }
    fn cloud(&self) -> bool {
        !self.email.trim().is_empty()
    }
    fn auth(&self) -> CmdResult<String> {
        let token = secret(&format!("jira:{}", self.base())).ok_or("Jira is not connected (Settings → Integrations)")?;
        Ok(if self.cloud() {
            format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{}:{token}", self.email.trim())))
        } else {
            format!("Bearer {token}")
        })
    }
    fn api(&self) -> &'static str {
        if self.cloud() { "/rest/api/3" } else { "/rest/api/2" }
    }
    fn get(&self, path: &str, query: &[(&str, &str)]) -> CmdResult<Value> {
        let mut req = agent().get(&format!("{}{}{path}", self.base(), self.api())).header("Authorization", &self.auth()?).header("Accept", "application/json").header("User-Agent", USER_AGENT);
        for (k, v) in query {
            req = req.query(*k, *v);
        }
        let (status, body) = send(req.call(), "Jira")?;
        if status != 200 {
            return Err(http_error("Jira", status, &body));
        }
        Ok(body)
    }
}

/// Plain text of an Atlassian Document Format value (Jira Cloud descriptions).
fn adf_text(node: &Value, out: &mut String) {
    let kind = node["type"].as_str().unwrap_or("");
    match kind {
        "text" => out.push_str(node["text"].as_str().unwrap_or("")),
        "hardBreak" => out.push('\n'),
        "mention" | "emoji" => out.push_str(node["attrs"]["text"].as_str().unwrap_or("")),
        "inlineCard" | "blockCard" => out.push_str(node["attrs"]["url"].as_str().unwrap_or("")),
        "media" | "mediaSingle" | "mediaGroup" => out.push_str("[attachment]"),
        _ => {}
    }
    let children = node["content"].as_array();
    if kind == "bulletList" || kind == "orderedList" {
        for (i, item) in children.into_iter().flatten().enumerate() {
            out.push_str(&if kind == "orderedList" { format!("{}. ", i + 1) } else { "- ".to_string() });
            adf_text(item, out);
        }
        return;
    }
    if kind == "tableRow" {
        let cells: Vec<String> = children
            .into_iter()
            .flatten()
            .map(|c| {
                let mut s = String::new();
                adf_text(c, &mut s);
                s.trim().replace('\n', " ")
            })
            .collect();
        out.push_str(&cells.join(" | "));
        out.push('\n');
        return;
    }
    for c in children.into_iter().flatten() {
        adf_text(c, out);
    }
    if matches!(kind, "paragraph" | "heading" | "codeBlock" | "blockquote" | "listItem" | "rule" | "panel") && !out.ends_with('\n') {
        out.push('\n');
    }
}

fn jira_kind(category: &str) -> &'static str {
    match category {
        "new" => "open",
        "indeterminate" => "progress",
        "done" => "done",
        _ => "other",
    }
}

fn jira_issue(site: &str, n: &Value) -> Issue {
    let f = &n["fields"];
    let description = match &f["description"] {
        Value::String(s) => clip(s),
        v @ Value::Object(_) => {
            let mut s = String::new();
            adf_text(v, &mut s);
            clip(&s)
        }
        _ => None,
    };
    let key = n["key"].as_str().unwrap_or_default().to_string();
    Issue {
        source: "jira",
        id: key.clone(),
        url: format!("{site}/browse/{key}"),
        key,
        title: f["summary"].as_str().unwrap_or_default().into(),
        state: f["status"]["name"].as_str().unwrap_or_default().into(),
        state_kind: jira_kind(f["status"]["statusCategory"]["key"].as_str().unwrap_or_default()).into(),
        priority: f["priority"]["name"].as_str().map(str::to_string),
        branch: None,
        description,
        team: f["project"]["key"].as_str().map(str::to_string),
        updated: f["updated"].as_str().map(str::to_string),
    }
}

fn jql_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[tauri::command]
pub async fn jira_issues(site: JiraSite, search: Option<String>, jql: Option<String>) -> CmdResult<Vec<Issue>> {
    crate::fs_ops::blocking(move || {
        let jql = match (jql.filter(|j| !j.trim().is_empty()), search.filter(|s| !s.trim().is_empty())) {
            (Some(j), _) => j,
            (None, Some(q)) => format!("text ~ {} ORDER BY updated DESC", jql_quote(q.trim())),
            (None, None) => "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC".into(),
        };
        let fields = "summary,status,description,priority,project,updated";
        // Cloud: the enhanced search (`/search` was removed); Server / DC: the classic one.
        let path = if site.cloud() { "/search/jql" } else { "/search" };
        let body = site.get(path, &[("jql", &jql), ("fields", fields), ("maxResults", "50")])?;
        Ok(body["issues"].as_array().into_iter().flatten().map(|n| jira_issue(&site.base(), n)).collect())
    })
    .await
}

#[tauri::command]
pub async fn jira_transitions(site: JiraSite, key: String) -> CmdResult<Vec<IssueState>> {
    crate::fs_ops::blocking(move || {
        let body = site.get(&format!("/issue/{}/transitions", urlencoding(&key)), &[])?;
        Ok(body["transitions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| IssueState {
                id: t["id"].as_str().unwrap_or_default().into(),
                name: t["name"].as_str().unwrap_or_default().into(),
                state_kind: jira_kind(t["to"]["statusCategory"]["key"].as_str().unwrap_or_default()).into(),
            })
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn jira_transition(site: JiraSite, key: String, transition: String) -> CmdResult<()> {
    crate::fs_ops::blocking(move || {
        let url = format!("{}{}/issue/{}/transitions", site.base(), site.api(), urlencoding(&key));
        let req = agent().post(&url).header("Authorization", &site.auth()?).header("User-Agent", USER_AGENT).send_json(json!({"transition": {"id": transition}}));
        let (status, body) = send(req, "Jira")?;
        if status == 204 || status == 200 { Ok(()) } else { Err(http_error("Jira", status, &body)) }
    })
    .await
}

#[tauri::command]
pub async fn jira_myself(site: JiraSite) -> CmdResult<String> {
    crate::fs_ops::blocking(move || {
        let me = site.get("/myself", &[])?;
        Ok(me["displayName"].as_str().or(me["name"].as_str()).unwrap_or("?").to_string())
    })
    .await
}

fn urlencoding(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

// ---------- GitLab ----------

/// Host and project path of a git remote URL (https, ssh or scp-like).
pub fn parse_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (host, path) = if let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) {
        // An HTTP(S) remote's port is the web server's: keep it.
        let rest = rest.split_once('@').map_or(rest, |(_, r)| r);
        let (host, path) = rest.split_once('/')?;
        (host.to_string(), path.to_string())
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        // An SSH port is not the API's.
        let rest = rest.split_once('@').map_or(rest, |(_, r)| r);
        let (host, path) = rest.split_once('/')?;
        (host.split(':').next()?.to_string(), path.to_string())
    } else {
        // scp-like: git@host:group/project.git
        let rest = url.split_once('@').map_or(url, |(_, r)| r);
        let (host, path) = rest.split_once(':')?;
        // `C:/repos/x` is a Windows path, not host `C`.
        if host.len() < 2 || host.contains(['/', '\\']) {
            return None;
        }
        (host.to_string(), path.to_string())
    };
    let path = path.trim_end_matches('/').trim_end_matches(".git").trim_matches('/').to_string();
    (!host.is_empty() && path.contains('/')).then_some((host.to_lowercase(), path))
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    // A remote project: the same git command on its host.
    if let Some(r) = root.to_str().and_then(crate::remote::parse_root) {
        let script = format!("cd {} && git {}", crate::remote::q(&r.path), args.iter().map(|a| crate::remote::q(a)).collect::<Vec<_>>().join(" "));
        return crate::remote::exec_blocking(&r, &script).filter(|s| !s.is_empty());
    }
    let mut cmd = std::process::Command::new("git");
    crate::daemon_runtime::clean_env(&mut cmd);
    cmd.arg("-C").arg(root).args(args).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

struct GitLabProject {
    host: String,
    path: String,
    token: Option<String>,
    /// `http` only for a remote that says so (a self-hosted test instance).
    http: bool,
}

impl GitLabProject {
    /// The project of `root`'s `origin` when it is on a GitLab host: gitlab.com, a host whose
    /// name says so, or one the user connected (`hosts`).
    fn of(root: &Path, hosts: &[String]) -> Option<GitLabProject> {
        let remote = git(root, &["remote", "get-url", "origin"])?;
        let (host, path) = parse_remote(&remote)?;
        let bare = |h: &str| h.trim_start_matches("http://").trim_start_matches("https://").to_string();
        let known = host == "gitlab.com" || host.contains("gitlab") || hosts.iter().any(|h| bare(h).eq_ignore_ascii_case(&host));
        known.then(|| GitLabProject { token: secret(&format!("gitlab:{host}")), http: remote.starts_with("http://"), host, path })
    }
    fn api(&self, path: &str) -> String {
        format!("{}://{}/api/v4{path}", if self.http { "http" } else { "https" }, self.host)
    }
    fn project(&self) -> String {
        format!("/projects/{}", urlencoding(&self.path))
    }
    fn get(&self, path: &str, query: &[(&str, &str)]) -> CmdResult<Value> {
        let mut req = agent().get(&self.api(path)).header("User-Agent", USER_AGENT);
        if let Some(t) = &self.token {
            req = req.header("PRIVATE-TOKEN", t);
        }
        for (k, v) in query {
            req = req.query(*k, *v);
        }
        let (status, body) = send(req.call(), "GitLab")?;
        if status != 200 {
            return Err(http_error("GitLab", status, &body));
        }
        Ok(body)
    }
}

#[derive(Serialize, Debug, PartialEq)]
pub struct MergeRequest {
    iid: u64,
    title: String,
    url: String,
    /// opened | merged | closed
    state: String,
    draft: bool,
    target: String,
    /// GitLab pipeline status (success, failed, running…) of the MR's head, if any.
    pipeline: Option<String>,
    merge_status: Option<String>,
}

fn merge_request(v: &Value) -> MergeRequest {
    MergeRequest {
        iid: v["iid"].as_u64().unwrap_or(0),
        title: v["title"].as_str().unwrap_or_default().into(),
        url: v["web_url"].as_str().unwrap_or_default().into(),
        state: v["state"].as_str().unwrap_or_default().into(),
        draft: v["draft"].as_bool().or(v["work_in_progress"].as_bool()).unwrap_or(false),
        target: v["target_branch"].as_str().unwrap_or_default().into(),
        pipeline: v["head_pipeline"]["status"].as_str().or(v["pipeline"]["status"].as_str()).map(str::to_string),
        merge_status: v["detailed_merge_status"].as_str().map(str::to_string),
    }
}

#[derive(Serialize, Debug)]
pub struct GitLabInfo {
    host: String,
    path: String,
    connected: bool,
}

/// Whether `root` is a GitLab project (and whether a token is set for its host).
#[tauri::command]
pub async fn gitlab_project(root: String, hosts: Option<Vec<String>>) -> CmdResult<Option<GitLabInfo>> {
    crate::fs_ops::blocking(move || {
        Ok(GitLabProject::of(Path::new(&root), &hosts.unwrap_or_default()).map(|p| GitLabInfo { connected: p.token.is_some(), host: p.host, path: p.path }))
    })
    .await
}

/// The merge request of the current branch (newest, any state).
#[tauri::command]
pub async fn gitlab_mr_status(root: String, hosts: Option<Vec<String>>) -> CmdResult<Option<MergeRequest>> {
    crate::fs_ops::blocking(move || {
        let root = Path::new(&root);
        let Some(p) = GitLabProject::of(root, &hosts.unwrap_or_default()) else { return Ok(None) };
        let Some(branch) = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| b != "HEAD") else { return Ok(None) };
        let list = p.get(&format!("{}/merge_requests", p.project()), &[("source_branch", &branch), ("state", "all"), ("order_by", "updated_at"), ("per_page", "5")])?;
        let Some(first) = list.as_array().and_then(|l| l.first()) else { return Ok(None) };
        // The list leaves out the pipeline; the single MR has it.
        let full = first["iid"].as_u64().and_then(|iid| p.get(&format!("{}/merge_requests/{iid}", p.project()), &[]).ok());
        Ok(Some(merge_request(full.as_ref().unwrap_or(first))))
    })
    .await
}

#[tauri::command]
pub async fn gitlab_default_branch(root: String, hosts: Option<Vec<String>>) -> CmdResult<Option<String>> {
    crate::fs_ops::blocking(move || {
        let Some(p) = GitLabProject::of(Path::new(&root), &hosts.unwrap_or_default()) else { return Ok(None) };
        Ok(p.get(&p.project(), &[])?["default_branch"].as_str().map(str::to_string))
    })
    .await
}

/// Opens a merge request for the current branch (which must be pushed). Returns its URL.
#[tauri::command]
pub async fn gitlab_mr_create(root: String, hosts: Option<Vec<String>>, title: String, description: String, target: String, draft: bool) -> CmdResult<String> {
    crate::fs_ops::blocking(move || {
        let root = Path::new(&root);
        let p = GitLabProject::of(root, &hosts.unwrap_or_default()).ok_or("this project's origin is not on GitLab")?;
        let token = p.token.clone().ok_or_else(|| format!("connect GitLab for {} first (Settings → Integrations)", p.host))?;
        let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| b != "HEAD").ok_or("check out a branch first")?;
        let title = if draft && !title.starts_with("Draft:") { format!("Draft: {title}") } else { title };
        let body = json!({"source_branch": branch, "target_branch": target, "title": title, "description": description, "remove_source_branch": true});
        let req = agent().post(&p.api(&format!("{}/merge_requests", p.project()))).header("PRIVATE-TOKEN", &token).header("User-Agent", USER_AGENT).send_json(body);
        let (status, v) = send(req, "GitLab")?;
        match status {
            201 => Ok(v["web_url"].as_str().unwrap_or_default().to_string()),
            409 => Err("a merge request for this branch already exists".into()),
            _ => Err(http_error("GitLab", status, &v)),
        }
    })
    .await
}

fn gitlab_issue(v: &Value) -> Issue {
    let state = v["state"].as_str().unwrap_or_default();
    Issue {
        source: "gitlab",
        id: v["iid"].as_u64().map(|i| i.to_string()).unwrap_or_default(),
        key: v["references"]["full"].as_str().map(str::to_string).unwrap_or_else(|| format!("#{}", v["iid"].as_u64().unwrap_or(0))),
        title: v["title"].as_str().unwrap_or_default().into(),
        url: v["web_url"].as_str().unwrap_or_default().into(),
        state: state.into(),
        state_kind: if state == "closed" { "done".into() } else { "open".into() },
        priority: None,
        branch: None,
        description: v["description"].as_str().and_then(clip),
        team: None,
        updated: v["updated_at"].as_str().map(str::to_string),
    }
}

/// Open issues assigned to the token's user (GitLab-wide), or a search in the project.
#[tauri::command]
pub async fn gitlab_issues(root: String, hosts: Option<Vec<String>>, search: Option<String>) -> CmdResult<Vec<Issue>> {
    crate::fs_ops::blocking(move || {
        let p = GitLabProject::of(Path::new(&root), &hosts.unwrap_or_default()).ok_or("this project's origin is not on GitLab")?;
        let list = match search.filter(|s| !s.trim().is_empty()) {
            Some(q) => p.get(&format!("{}/issues", p.project()), &[("search", q.trim()), ("state", "opened"), ("per_page", "50")])?,
            None => {
                if p.token.is_none() {
                    return Err(format!("connect GitLab for {} to list the issues assigned to you", p.host));
                }
                p.get("/issues", &[("scope", "assigned_to_me"), ("state", "opened"), ("per_page", "50")])?
            }
        };
        Ok(list.as_array().into_iter().flatten().map(gitlab_issue).collect())
    })
    .await
}

/// Who a GitLab token belongs to.
#[tauri::command]
pub async fn gitlab_user(host: String) -> CmdResult<String> {
    crate::fs_ops::blocking(move || {
        let http = host.starts_with("http://");
        let host = host.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/').to_string();
        let p = GitLabProject { token: secret(&format!("gitlab:{host}")), host, path: String::new(), http };
        if p.token.is_none() {
            return Err("no token".into());
        }
        let me = p.get("/user", &[])?;
        Ok(format!("{} (@{})", me["name"].as_str().unwrap_or("?"), me["username"].as_str().unwrap_or("?")))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remotes() {
        let want = Some(("gitlab.com".to_string(), "group/sub/proj".to_string()));
        assert_eq!(parse_remote("git@gitlab.com:group/sub/proj.git"), want);
        assert_eq!(parse_remote("https://gitlab.com/group/sub/proj.git"), want);
        assert_eq!(parse_remote("https://user@GitLab.com/group/sub/proj/"), want);
        assert_eq!(parse_remote("ssh://git@gitlab.com:2222/group/sub/proj.git"), want);
        assert_eq!(parse_remote("http://127.0.0.1:8123/group/sub/proj.git"), Some(("127.0.0.1:8123".to_string(), "group/sub/proj".to_string())));
        assert_eq!(parse_remote("C:/repos/local"), None);
        assert_eq!(parse_remote("https://example.com/single"), None);
    }

    #[test]
    fn flattens_adf() {
        let doc = json!({"type": "doc", "content": [
            {"type": "heading", "content": [{"type": "text", "text": "Steps"}]},
            {"type": "orderedList", "content": [
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Open the page"}]}]},
                {"type": "listItem", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Click "}, {"type": "mention", "attrs": {"text": "@ann"}}]}]}
            ]},
            {"type": "paragraph", "content": [{"type": "text", "text": "see"}, {"type": "hardBreak"}, {"type": "inlineCard", "attrs": {"url": "https://x.test"}}]}
        ]});
        let mut s = String::new();
        adf_text(&doc, &mut s);
        assert_eq!(s, "Steps\n1. Open the page\n2. Click @ann\nsee\nhttps://x.test\n");
    }

    #[test]
    fn maps_issues() {
        let l = linear_issue(&json!({"id": "u1", "identifier": "ENG-1", "title": "T", "url": "https://linear.app/x/issue/ENG-1", "branchName": "me/eng-1-t", "priorityLabel": "High", "state": {"name": "In Progress", "type": "started"}, "team": {"key": "ENG"}, "description": "  body  "}));
        assert_eq!((l.key.as_str(), l.state_kind.as_str(), l.branch.as_deref(), l.description.as_deref(), l.priority.as_deref()), ("ENG-1", "progress", Some("me/eng-1-t"), Some("body"), Some("High")));
        let j = jira_issue("https://x.atlassian.net", &json!({"key": "P-7", "fields": {"summary": "S", "status": {"name": "To Do", "statusCategory": {"key": "new"}}, "description": "plain v2 text", "project": {"key": "P"}}}));
        assert_eq!((j.url.as_str(), j.state_kind.as_str(), j.description.as_deref()), ("https://x.atlassian.net/browse/P-7", "open", Some("plain v2 text")));
        let m = merge_request(&json!({"iid": 4, "title": "Draft: x", "web_url": "u", "state": "opened", "draft": true, "target_branch": "main", "head_pipeline": {"status": "failed"}, "detailed_merge_status": "ci_must_pass"}));
        assert_eq!((m.iid, m.draft, m.pipeline.as_deref(), m.merge_status.as_deref()), (4, true, Some("failed"), Some("ci_must_pass")));
        let g = gitlab_issue(&json!({"iid": 9, "title": "Bug", "web_url": "w", "state": "opened", "references": {"full": "grp/p#9"}}));
        assert_eq!((g.key.as_str(), g.state_kind.as_str()), ("grp/p#9", "open"));
    }

    #[test]
    fn quotes_jql_text() {
        assert_eq!(jql_quote(r#"say "hi" \ now"#), r#""say \"hi\" \\ now""#);
    }

    #[test]
    fn only_known_secret_keys() {
        assert!(valid_key("linear").is_ok());
        assert!(valid_key("jira:https://x.atlassian.net").is_ok());
        assert!(valid_key("gitlab:gitlab.com").is_ok());
        assert!(valid_key("claude").is_err());
        assert!(valid_key("linear\nx").is_err());
    }

    /// `cargo test -p kelpie --lib -- --ignored live_gitlab` — reads a public GitLab project's
    /// merge requests without a token (network).
    #[test]
    #[ignore]
    fn live_gitlab_public_project() {
        let p = GitLabProject { host: "gitlab.com".into(), path: "gitlab-org/cli".into(), token: None, http: false };
        let list = p.get(&format!("{}/merge_requests", p.project()), &[("state", "merged"), ("per_page", "2")]).unwrap();
        let first = &list.as_array().unwrap()[0];
        let mr = merge_request(&p.get(&format!("{}/merge_requests/{}", p.project(), first["iid"]), &[]).unwrap());
        println!("{mr:?}");
        assert_eq!(mr.state, "merged");
    }
}
