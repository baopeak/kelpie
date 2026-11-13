//! What an agent sees of a page, as plain functions: refs (`e12`) for the elements the engine (`browser_agent.js`) reports, the snapshot
//! as text, what changed after an action, and which sites a pane may be on.
//!
//! The engine names an element by a number that is good only in its own document (a random token per document). A pane keeps one
//! counter across documents, so a ref names one element of one document: the same element keeps its ref from snapshot to snapshot, and
//! a ref never reaches an element of another document.

use std::collections::{HashMap, VecDeque};
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::Url;

/// One element as the engine reports it. `id` 0 is text, which has no ref.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Node {
    pub id: u64,
    pub depth: u32,
    pub role: String,
    pub name: String,
    pub flags: Vec<String>,
    pub value: Option<String>,
    pub href: Option<String>,
}

/// A snapshot as the engine reports it: `more` elements it left out to stay near the size asked for.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Snapshot {
    pub doc: String,
    pub url: String,
    pub title: String,
    pub nodes: Vec<Node>,
    pub more: usize,
}

pub const STALE: &str = "stale ref: the page changed; take a new snapshot";

/// Documents a pane remembers refs for (going back may bring one back as it was).
const DOCS: usize = 4;
/// Refs one document keeps; past it, the ones shown least recently are forgotten (a long-lived page keeps changing).
const PER_DOC: usize = 5_000;

#[derive(Debug, Clone, Copy)]
struct Entry {
    doc: u32,
    id: u64,
    /// When the ref was last given out.
    seen: u64,
}

/// The refs of one pane. Documents are known by a short number of their own.
#[derive(Default, Debug)]
pub struct Refs {
    last: u64,
    tick: u64,
    docs: VecDeque<(String, u32)>,
    next_doc: u32,
    to_ref: HashMap<(u32, u64), u64>,
    to_el: HashMap<u64, Entry>,
    per_doc: HashMap<u32, usize>,
}

impl Refs {
    fn doc_of(&self, doc: &str) -> Option<u32> {
        self.docs.iter().find(|(d, _)| d == doc).map(|(_, n)| *n)
    }

    fn doc_no(&mut self, doc: &str) -> u32 {
        if let Some(n) = self.doc_of(doc) {
            return n;
        }
        let n = self.next_doc;
        self.next_doc += 1;
        self.docs.push_back((doc.to_string(), n));
        while self.docs.len() > DOCS {
            let Some((_, old)) = self.docs.pop_front() else { break };
            self.to_ref.retain(|(d, _), _| *d != old);
            self.to_el.retain(|_, e| e.doc != old);
            self.per_doc.remove(&old);
        }
        n
    }

    /// The ref of element `id` of document `doc`, made the first time it is asked for.
    pub fn name(&mut self, doc: &str, id: u64) -> String {
        let d = self.doc_no(doc);
        self.tick += 1;
        if let Some(&n) = self.to_ref.get(&(d, id)) {
            if let Some(e) = self.to_el.get_mut(&n) {
                e.seen = self.tick;
            }
            return format!("e{n}");
        }
        self.last += 1;
        self.to_ref.insert((d, id), self.last);
        self.to_el.insert(self.last, Entry { doc: d, id, seen: self.tick });
        let count = self.per_doc.entry(d).or_default();
        *count += 1;
        if *count > PER_DOC {
            // Down to nine tenths at once, the ones shown least recently first.
            let mut mine: Vec<(u64, u64)> = self.to_el.iter().filter(|(_, e)| e.doc == d).map(|(n, e)| (e.seen, *n)).collect();
            mine.sort_unstable();
            let drop = mine.len() - PER_DOC * 9 / 10;
            for (_, n) in &mine[..drop] {
                if let Some(e) = self.to_el.remove(n) {
                    self.to_ref.remove(&(e.doc, e.id));
                }
            }
            self.per_doc.insert(d, mine.len() - drop);
        }
        format!("e{}", self.last)
    }

    /// The engine's id of ref `r` in document `doc` (the page's now). A ref of another document, or one forgotten, is stale.
    pub fn resolve(&self, r: &str, doc: &str) -> Result<u64, String> {
        let n: u64 = r.strip_prefix('e').and_then(|n| n.parse().ok()).ok_or_else(|| format!("`{r}` is not a ref: refs look like e12"))?;
        match self.to_el.get(&n) {
            Some(e) if self.doc_of(doc) == Some(e.doc) => Ok(e.id),
            Some(_) => Err(STALE.into()),
            None if n >= 1 && n <= self.last => Err(STALE.into()),
            None => Err(format!("there is no element {r}: take a snapshot to get refs")),
        }
    }
}

fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// Labels of fields whose value is never shown, should the engine not have said so: one-time codes, card numbers, PINs, keys.
static SECRET_LABEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(passw|passcode|secret|token|api[-_ ]?key|otp|one[-_ ]?time|mfa|2fa|cvv|cvc|card|cc-|ccnum|(^|[^a-z])pin([^a-z]|$)|ssn|security.?code|credential)").unwrap()
});
/// Values that look like a key or a token.
static SECRET_VALUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:[A-Za-z0-9+/_-]{32,}={0,2}|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]*)$").unwrap());
/// Query parameters whose values never leave the app: tokens, codes and the sign-in state of an OAuth window.
static SECRET_PARAM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(token|secret|passw|api[-_]?key|session|csrf|xsrf|auth|signature|sig$|credential|cookie|otp|^code$|^state$|^nonce$|login_hint)").unwrap()
});

fn flagged(n: &Node) -> bool {
    n.flags.iter().any(|f| f == "password" || f == "secret")
}

/// Whether the element's value is never shown: the engine marked it so, or it is a field whose label or value looks secret.
pub fn secret(n: &Node) -> bool {
    flagged(n)
        || (matches!(n.role.as_str(), "textbox" | "searchbox" | "combobox" | "spinbutton")
            && (SECRET_LABEL.is_match(&n.name) || n.value.as_deref().is_some_and(|v| SECRET_VALUE.is_match(v))))
}

/// An address with the values of secret-looking query parameters and an OAuth fragment taken out.
pub fn scrub_url(u: &str) -> String {
    let Ok(mut url) = Url::parse(u) else { return String::new() };
    let hidden = |k: &str, v: &str| SECRET_PARAM.is_match(k) || SECRET_VALUE.is_match(v);
    let pairs: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    if pairs.iter().any(|(k, v)| hidden(k, v)) {
        let clean: Vec<(String, String)> = pairs.into_iter().map(|(k, v)| if hidden(&k, &v) { (k, "[redacted]".to_string()) } else { (k, v) }).collect();
        url.query_pairs_mut().clear().extend_pairs(clean);
    }
    if url.fragment().is_some_and(|f| f.contains("access_token") || f.contains("id_token") || f.contains("code=")) {
        url.set_fragment(Some("[redacted]"));
    }
    url.to_string()
}

/// One element on one line, without its indent: `<role> "<name>" [ref=eN] [flags] value="…" -> <path>`. `masked`: its value is left out
/// (the field the agent just typed into); `focus`: the `focused` flag is kept.
pub fn line(n: &Node, r: Option<&str>, masked: bool, focus: bool) -> String {
    let mut s = n.role.clone();
    if !n.name.is_empty() {
        s.push(' ');
        s.push_str(&quoted(&n.name));
    }
    if let Some(r) = r {
        s.push_str(&format!(" [ref={r}]"));
    }
    for f in n.flags.iter().filter(|f| focus || *f != "focused") {
        s.push_str(&format!(" [{f}]"));
    }
    let secret = secret(n);
    if secret && !flagged(n) {
        s.push_str(" [secret]");
    }
    if let Some(v) = n.value.as_deref().filter(|_| !masked && !secret) {
        s.push_str(&format!(" value={}", quoted(v)));
    }
    if let Some(h) = n.href.as_deref().filter(|h| !h.is_empty()) {
        s.push_str(&format!(" -> {h}"));
    }
    s
}
/// A snapshot as the agent reads it.
pub struct Rendered {
    pub text: String,
    /// The elements shown, as data (`structuredContent`).
    pub elements: Vec<Value>,
    pub truncated: bool,
}

/// The snapshot as indented lines, at most `max_chars`, then how many were left out. The value of element `mask` is left out.
pub fn render(s: &Snapshot, refs: &mut Refs, max_chars: usize, mask: Option<u64>) -> Rendered {
    let mut text = String::new();
    let mut elements = Vec::new();
    for n in &s.nodes {
        let r = (n.id != 0).then(|| refs.name(&s.doc, n.id));
        let masked = mask.is_some_and(|m| m == n.id && m != 0);
        let l = format!("{}- {}", "  ".repeat(n.depth.min(40) as usize), line(n, r.as_deref(), masked, true));
        if text.len() + l.len() + 1 > max_chars {
            break;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&l);
        let mut e = json!({"role": n.role, "name": n.name, "depth": n.depth});
        if let Some(r) = r {
            e["ref"] = json!(r);
        }
        if !n.flags.is_empty() {
            e["flags"] = json!(n.flags);
        }
        if let Some(v) = n.value.as_deref().filter(|_| !masked && !secret(n)) {
            e["value"] = json!(v);
        }
        if let Some(h) = &n.href {
            e["href"] = json!(h);
        }
        elements.push(e);
    }
    let more = s.nodes.len() - elements.len() + s.more;
    if more > 0 {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!("… {more} more (raise max_chars or use root)"));
    } else if text.is_empty() {
        text.push_str("(no visible elements)");
    }
    Rendered { text, elements, truncated: more > 0 }
}

/// The snapshot as unindented lines for `diff`, with refs; the value of element `mask` is left out.
pub fn lines(s: &Snapshot, refs: &mut Refs, mask: Option<u64>) -> Vec<String> {
    s.nodes
        .iter()
        .map(|n| {
            let r = (n.id != 0).then(|| refs.name(&s.doc, n.id));
            line(n, r.as_deref(), mask.is_some_and(|m| m == n.id && m != 0), false)
        })
        .collect()
}

/// What changed from `before` to `after`: lines gone (`- `) and lines new (`+ `), at most `max` of them shared between the two, then how
/// many more there were.
pub fn diff(before: &[String], after: &[String], max: usize) -> Vec<String> {
    let mut left: HashMap<&str, usize> = HashMap::new();
    for l in before {
        *left.entry(l.as_str()).or_default() += 1;
    }
    let mut added = Vec::new();
    for l in after {
        match left.get_mut(l.as_str()) {
            Some(c) if *c > 0 => *c -= 1,
            _ => added.push(l.as_str()),
        }
    }
    let mut removed = Vec::new();
    for l in before {
        if let Some(c) = left.get_mut(l.as_str()).filter(|c| **c > 0) {
            *c -= 1;
            removed.push(l.as_str());
        }
    }
    let take_removed = removed.len().min(max.saturating_sub(added.len().min(max / 2)));
    let take_added = added.len().min(max - take_removed);
    let mut out: Vec<String> = removed[..take_removed].iter().map(|l| format!("- {l}")).collect();
    out.extend(added[..take_added].iter().map(|l| format!("+ {l}")));
    let rest = removed.len() + added.len() - take_removed - take_added;
    if rest > 0 {
        out.push(format!("… {rest} more changes (take a snapshot)"));
    }
    out
}

// ---------- sites ----------

/// How a pane came to be usable by the session: opened for it (an agent's page) or shared by the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Opened,
    Shared,
}

/// This computer's own sites: localhost (and names under it), 127.0.0.0/8 and [::1], any port, http or https.
pub fn loopback(url: &Url) -> bool {
    let Some(host) = url.host_str() else { return false };
    host == "localhost" || host.ends_with(".localhost") || host.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Whether agents may use a pane whose page is at `url`: a pane opened for agents with a profile of its own (`isolated`) on any http(s)
/// site, one without on this computer's own sites and the user's allowed `sites`; a shared pane on the sites it was shared on
/// (`shared`) and this computer's own. An empty page counts as allowed: there is nothing on it.
pub fn site_allowed(url: &str, kind: Kind, isolated: bool, shared: &[String], sites: &[String]) -> Result<(), String> {
    if url == "about:blank" {
        return Ok(());
    }
    let parsed = Url::parse(url).ok().filter(|u| matches!(u.scheme(), "http" | "https"));
    let Some(u) = parsed else {
        return Err("the pane does not show a web page agents may use (only http and https pages)".into());
    };
    // The page's own address is never repeated in the error: it may be a site the agent must not learn of.
    let origin = u.origin().ascii_serialization();
    let ok = loopback(&u)
        || match kind {
            Kind::Opened => isolated || sites.iter().any(|s| *s == origin),
            Kind::Shared => shared.iter().any(|s| *s == origin),
        };
    if ok {
        return Ok(());
    }
    Err(match kind {
        Kind::Shared => format!(
            "This pane was shared on {}; the page is now on another site, which agents may not use. Ask the user to share the pane again there.",
            if shared.is_empty() { "no site".to_string() } else { shared.join(", ") }
        ),
        Kind::Opened => "In this pane agents may use only this computer's own sites (localhost) and the sites allowed in Kelpie → Settings → Agents → Browser panes; that site is not one of them.".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64, depth: u32, role: &str, name: &str) -> Node {
        Node { id, depth, role: role.into(), name: name.into(), ..Node::default() }
    }

    fn snap(doc: &str, nodes: Vec<Node>) -> Snapshot {
        Snapshot { doc: doc.into(), url: "http://localhost:5173/".into(), title: "T".into(), nodes, more: 0 }
    }

    #[test]
    fn an_element_keeps_its_ref_across_snapshots_of_its_document() {
        let mut refs = Refs::default();
        let a = refs.name("d1", 7);
        let b = refs.name("d1", 9);
        assert_eq!((a.as_str(), b.as_str()), ("e1", "e2"));
        assert_eq!(refs.name("d1", 7), "e1");
        assert_eq!(refs.resolve("e2", "d1"), Ok(9));
        // The same engine id in a new document is another element, with a new ref.
        assert_eq!(refs.name("d2", 7), "e3");
    }

    #[test]
    fn a_ref_never_resolves_in_another_document() {
        let mut refs = Refs::default();
        refs.name("d1", 1);
        assert_eq!(refs.resolve("e1", "d2"), Err(STALE.to_string()));
        assert!(refs.resolve("e9", "d1").unwrap_err().contains("no element e9"));
        assert!(refs.resolve("x1", "d1").is_err());
        // Old documents are forgotten; their refs stay stale and numbers are never used again.
        for d in 2..=10 {
            refs.name(&format!("d{d}"), 1);
        }
        assert_eq!(refs.resolve("e1", "d1"), Err(STALE.to_string()));
        assert_eq!(refs.name("d1", 1), "e11");
    }

    #[test]
    fn the_snapshot_is_cut_at_max_chars_and_says_how_many_more() {
        let nodes: Vec<Node> = (1..=50).map(|i| node(i, 0, "button", &format!("Button {i}"))).collect();
        let mut s = snap("d", nodes);
        s.more = 5;
        let mut refs = Refs::default();
        let r = render(&s, &mut refs, 200, None);
        assert!(r.truncated);
        let body: Vec<&str> = r.text.lines().collect();
        let shown = body.len() - 1;
        assert!(body[..shown].join("\n").len() <= 200);
        assert_eq!(body[0], "- button \"Button 1\" [ref=e1]");
        assert_eq!(*body.last().unwrap(), format!("… {} more (raise max_chars or use root)", 50 - shown + 5));
        assert_eq!(r.elements.len(), shown);
        let all = render(&snap("d", vec![node(1, 0, "heading", "Hi"), node(2, 1, "link", "Docs")]), &mut Refs::default(), 8000, None);
        assert!(!all.truncated);
        assert_eq!(all.text, "- heading \"Hi\" [ref=e1]\n  - link \"Docs\" [ref=e2]");
        assert_eq!(render(&snap("d", vec![]), &mut Refs::default(), 100, None).text, "(no visible elements)");
    }

    #[test]
    fn lines_show_flags_values_and_paths() {
        let mut n = node(3, 0, "textbox", "Email");
        n.flags = vec!["required".into(), "focused".into()];
        n.value = Some("a\"b".into());
        assert_eq!(line(&n, Some("e3"), false, true), "textbox \"Email\" [ref=e3] [required] [focused] value=\"a\\\"b\"");
        assert_eq!(line(&n, Some("e3"), true, false), "textbox \"Email\" [ref=e3] [required]");
        let mut l = node(4, 0, "link", "Docs");
        l.href = Some("/docs?x=1".into());
        assert_eq!(line(&l, None, false, false), "link \"Docs\" -> /docs?x=1");
        assert_eq!(line(&node(0, 0, "text", ""), None, false, false), "text");
    }

    #[test]
    fn a_secret_field_never_shows_a_value() {
        for flag in ["password", "secret"] {
            let mut n = node(1, 0, "textbox", "Password");
            n.flags = vec![flag.into()];
            // Even if a value came along, it is not printed nor given as data.
            n.value = Some("hunter2".into());
            let l = line(&n, Some("e1"), false, true);
            assert_eq!(l, format!("textbox \"Password\" [ref=e1] [{flag}]"));
            let r = render(&snap("d", vec![n.clone()]), &mut Refs::default(), 8000, None);
            assert!(!r.text.contains("hunter2") && !r.elements[0].to_string().contains("hunter2"));
            assert!(!lines(&snap("d", vec![n]), &mut Refs::default(), None).concat().contains("hunter2"));
        }
    }

    #[test]
    fn a_long_lived_document_keeps_the_refs_it_shows_and_forgets_the_oldest() {
        let mut refs = Refs::default();
        assert_eq!(refs.name("d", 1), "e1");
        for id in 2..=PER_DOC as u64 {
            refs.name("d", id);
        }
        // Element 1 is shown again, so it is the newest; one more element goes past the cap.
        assert_eq!(refs.name("d", 1), "e1");
        refs.name("d", PER_DOC as u64 + 1);
        assert_eq!(refs.resolve("e1", "d"), Ok(1));
        assert_eq!(refs.resolve("e2", "d"), Err(STALE.to_string()));
        assert_eq!(refs.resolve(&format!("e{}", PER_DOC + 1), "d"), Ok(PER_DOC as u64 + 1));
        assert!(refs.to_el.len() <= PER_DOC);
    }

    #[test]
    fn the_typed_field_s_value_is_left_out_of_a_full_render_too() {
        let mut n = node(5, 0, "textbox", "Comment");
        n.value = Some("hello there".into());
        let r = render(&snap("d", vec![n.clone()]), &mut Refs::default(), 8000, Some(5));
        assert!(!r.text.contains("hello") && !r.elements[0].to_string().contains("hello"));
        assert!(render(&snap("d", vec![n]), &mut Refs::default(), 8000, None).text.contains("hello"));
    }

    #[test]
    fn fields_that_look_secret_never_show_a_value() {
        for (name, value) in [("One-time code", "123456"), ("CVV", "123"), ("Card number", "4111111111111111"), ("PIN", "0000"), ("API key", "kx9"), ("Notes", concat!("sk_", "live_", "51HxQfE2eZvKYlo2C", "0aBcDeFgHiJkLmNoP"))] {
            let mut n = node(1, 0, "textbox", name);
            n.value = Some(value.into());
            assert!(secret(&n), "{name}");
            let l = line(&n, None, false, true);
            assert!(l.ends_with("[secret]") && !l.contains(value), "{l}");
        }
        for name in ["Shipping address", "Spinner speed", "Email"] {
            let mut n = node(1, 0, "textbox", name);
            n.value = Some("ann".into());
            assert!(!secret(&n), "{name}");
        }
        // Only fields: a button may say "Card".
        assert!(!secret(&node(1, 0, "button", "Pay by card")));
    }

    #[test]
    fn secret_parts_of_an_address_are_taken_out() {
        let u = scrub_url("https://login.example.com/authorize?client_id=app&state=xyz123&login_hint=ann%40x.com&scope=openid#access_token=abc");
        assert!(u.starts_with("https://login.example.com/authorize?client_id=app&state=%5Bredacted%5D&login_hint=%5Bredacted%5D&scope=openid"), "{u}");
        assert!(u.contains("#") && u.rsplit('#').next().is_some_and(|f| f.contains("redacted")) && !u.contains("xyz123") && !u.contains("abc"), "{u}");
        assert_eq!(scrub_url("https://example.com/a?q=1"), "https://example.com/a?q=1");
        assert_eq!(scrub_url("not a url"), "");
    }

    #[test]
    fn the_diff_lists_what_went_and_what_came() {
        let before = vec!["a".to_string(), "b".into(), "c".into()];
        let after = vec!["a".to_string(), "c".into(), "d".into()];
        assert_eq!(diff(&before, &after, 20), ["- b", "+ d"]);
        assert!(diff(&before, &before, 20).is_empty());
        // Repeated lines count one by one.
        assert_eq!(diff(&["x".to_string(), "x".into()], &["x".to_string()], 20), ["- x"]);
        // Past the cap: room shared between the two, and the rest counted.
        let many: Vec<String> = (0..30).map(|i| format!("n{i}")).collect();
        let out = diff(&many, &[], 20);
        assert_eq!(out.len(), 21);
        assert_eq!(out[20], "… 10 more changes (take a snapshot)");
        let out = diff(&many[..15], &many[15..], 20);
        assert_eq!(out.iter().filter(|l| l.starts_with("- ")).count(), 10);
        assert_eq!(out.iter().filter(|l| l.starts_with("+ ")).count(), 10);
    }

    #[test]
    fn the_typed_field_s_value_stays_out_of_the_diff() {
        let mut n = node(5, 0, "textbox", "Email");
        let mut refs = Refs::default();
        let before = lines(&snap("d", vec![n.clone()]), &mut refs, Some(5));
        n.value = Some("ann@example.com".into());
        n.flags = vec!["focused".into()];
        let after = lines(&snap("d", vec![n]), &mut refs, Some(5));
        assert!(diff(&before, &after, 20).is_empty());
    }

    #[test]
    fn loopback_in_every_form_and_on_any_port_is_this_computer() {
        for u in ["http://localhost:5173/", "https://localhost/", "http://app.localhost:3000/x", "http://127.0.0.1:8080/", "http://127.1.2.3/", "http://[::1]:9000/"] {
            assert!(loopback(&Url::parse(u).unwrap()), "{u}");
            for (kind, isolated) in [(Kind::Opened, false), (Kind::Shared, false)] {
                assert_eq!(site_allowed(u, kind, isolated, &[], &[]), Ok(()), "{u}");
            }
        }
        for u in ["http://localhost.example.com/", "http://10.0.0.1/", "http://0.0.0.0:3000/", "http://example.com/"] {
            assert!(!loopback(&Url::parse(u).unwrap()), "{u}");
        }
    }

    #[test]
    fn an_opened_pane_without_its_own_profile_stays_on_allowed_sites() {
        let sites = vec!["https://staging.example.com".to_string()];
        assert_eq!(site_allowed("https://staging.example.com/login?x=1", Kind::Opened, false, &[], &sites), Ok(()));
        // Origins compare whole: another scheme or port is another site.
        assert!(site_allowed("http://staging.example.com/", Kind::Opened, false, &[], &sites).is_err());
        assert!(site_allowed("https://staging.example.com:8443/", Kind::Opened, false, &[], &sites).is_err());
        // The page's own site is not named: it may be one the agent must not learn of.
        assert!(!site_allowed("https://example.com/", Kind::Opened, false, &[], &sites).unwrap_err().contains("example.com"));
        // With a profile of its own, any web site.
        assert_eq!(site_allowed("https://example.com/", Kind::Opened, true, &[], &[]), Ok(()));
        assert!(site_allowed("file:///C:/x", Kind::Opened, true, &[], &[]).is_err());
        assert_eq!(site_allowed("about:blank", Kind::Opened, false, &[], &[]), Ok(()));
    }

    #[test]
    fn a_shared_pane_stays_on_the_site_it_was_shared_on() {
        let shared = vec!["https://app.example.com".to_string()];
        assert_eq!(site_allowed("https://app.example.com/a/b", Kind::Shared, false, &shared, &[]), Ok(()));
        assert_eq!(site_allowed("http://localhost:3000/", Kind::Shared, false, &shared, &[]), Ok(()));
        let e = site_allowed("https://evil.example.net/", Kind::Shared, false, &shared, &[]).unwrap_err();
        assert!(e.contains("shared on https://app.example.com") && !e.contains("evil"));
        // The user's allowed sites are for opened panes only, and isolation does not widen a shared pane.
        assert!(site_allowed("https://evil.example.net/", Kind::Shared, true, &shared, &["https://evil.example.net".into()]).is_err());
        // Addresses compare by their origin as a browser writes it: lower case, default port left out.
        assert_eq!(site_allowed("https://App.Example.com:443/x", Kind::Shared, false, &shared, &[]), Ok(()));
        assert!(site_allowed("data:text/html,x", Kind::Shared, false, &shared, &[]).is_err());
    }
}
