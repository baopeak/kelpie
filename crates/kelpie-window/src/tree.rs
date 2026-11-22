//! What an agent reads of a window: an outline of the accessibility tree, one line a node, and the actions a node takes.
//!
//! A node an agent can act on carries an id: UI Automation's runtime id, which names one element for as long as it exists. An id from an
//! earlier outline that no longer names an element is refused (`STALE`); nothing is guessed from position or name.
//!
//! The value of a password field is never read. Text an agent sets is not written anywhere but into the element.

/// Nodes in one outline. A window with more says how many were left out.
pub const MAX_NODES: usize = 400;
/// Elements read from one window before the walk stops, kept or not.
pub const MAX_WALK: usize = 6000;
pub const MAX_DEPTH: u32 = 64;
pub const MAX_NAME: usize = 120;
pub const MAX_VALUE: usize = 200;
/// A runtime id is a few integers; a longer one is not taken.
pub const MAX_ID_PARTS: usize = 16;
/// Text one `set` may carry.
pub const MAX_TEXT: usize = 4096;

pub const STALE: &str = "stale id: the window changed; read the tree again";

/// What makes a node something to act on; a node with one of these carries its id in the outline.
pub const ACTIONS: &[&str] = &["invoke", "set", "toggle", "expand", "select"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub depth: u32,
    /// The runtime id; empty when the element gave none.
    pub id: Vec<i32>,
    pub role: &'static str,
    pub name: String,
    pub value: Option<String>,
    /// Actions (`ACTIONS`) first, then states: `disabled`, `offscreen`, `focused`, `checked`, `unchecked`, `mixed`, `expanded`,
    /// `collapsed`, `selected`, `password`, `readonly`.
    pub flags: Vec<&'static str>,
    /// Where it is in the window's picture: x, y, width, height.
    pub at: Option<[i32; 4]>,
}

impl Node {
    pub fn actionable(&self) -> bool {
        self.flags.iter().any(|f| ACTIONS.contains(f))
    }
}

/// UI Automation's control types (`UIA_*ControlTypeId`, 50000 on) as the words an agent reads.
pub fn role(control_type: i32) -> &'static str {
    const ROLES: [&str; 41] = [
        "button",
        "calendar",
        "checkbox",
        "combobox",
        "edit",
        "link",
        "image",
        "listitem",
        "list",
        "menu",
        "menubar",
        "menuitem",
        "progressbar",
        "radio",
        "scrollbar",
        "slider",
        "spinner",
        "statusbar",
        "tab",
        "tabitem",
        "text",
        "toolbar",
        "tooltip",
        "tree",
        "treeitem",
        "custom",
        "group",
        "thumb",
        "datagrid",
        "dataitem",
        "document",
        "splitbutton",
        "window",
        "pane",
        "header",
        "headeritem",
        "table",
        "titlebar",
        "separator",
        "semanticzoom",
        "appbar",
    ];
    control_type
        .checked_sub(50000)
        .and_then(|i| ROLES.get(i as usize))
        .copied()
        .unwrap_or("element")
}

/// One line of text from what an app reports: no control characters, runs of white space as one space, at most `max` characters.
pub fn clean(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() || c.is_control() {
            space = !out.is_empty();
            continue;
        }
        if n + usize::from(space) >= max {
            out.push('…');
            return out;
        }
        if space {
            out.push(' ');
            n += 1;
            space = false;
        }
        out.push(c);
        n += 1;
    }
    out
}

/// Containers that say nothing (no name, no value, nothing to act on) are left out and their children move up, so an outline is the
/// app's content and not its layout. `nodes` is a tree in document order.
pub fn prune(nodes: Vec<Node>) -> Vec<Node> {
    const STRUCTURAL: &[&str] = &["pane", "group", "custom", "element"];
    let mut dropped: Vec<u32> = Vec::new();
    let mut out = Vec::with_capacity(nodes.len());
    for (i, mut n) in nodes.into_iter().enumerate() {
        while dropped.last().is_some_and(|d| *d >= n.depth) {
            dropped.pop();
        }
        // The first node is the window itself.
        let silent = i > 0
            && STRUCTURAL.contains(&n.role)
            && n.name.is_empty()
            && n.value.is_none()
            && !n.actionable();
        if silent {
            dropped.push(n.depth);
            continue;
        }
        n.depth -= dropped.len() as u32;
        out.push(n);
    }
    out
}

pub fn id_text(id: &[i32]) -> String {
    id.iter().map(i32::to_string).collect::<Vec<_>>().join(".")
}

pub fn parse_id(s: &str) -> Option<Vec<i32>> {
    if s.is_empty() || s.len() > 200 {
        return None;
    }
    let parts: Option<Vec<i32>> = s.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| p.len() <= MAX_ID_PARTS)
}

/// The outline: a node a line, two spaces a level. `[id]` is there for a node an agent can act on. `rects` adds where each node is in
/// the window's picture (`@x,y wxh`).
pub fn outline(nodes: &[Node], max: usize, rects: bool) -> String {
    let mut out = String::new();
    for n in nodes.iter().take(max) {
        for _ in 0..n.depth {
            out.push_str("  ");
        }
        out.push_str(n.role);
        if !n.name.is_empty() {
            out.push_str(&format!(" \"{}\"", n.name.replace('"', "'")));
        }
        if n.actionable() && !n.id.is_empty() {
            out.push_str(&format!(" [{}]", id_text(&n.id)));
        }
        if let Some(v) = &n.value {
            out.push_str(&format!(" =\"{}\"", v.replace('"', "'")));
        }
        if !n.flags.is_empty() {
            out.push_str(&format!(" ({})", n.flags.join(", ")));
        }
        if let (true, Some([x, y, w, h])) = (rects, n.at) {
            out.push_str(&format!(" @{x},{y} {w}x{h}"));
        }
        out.push('\n');
    }
    if nodes.len() > max {
        out.push_str(&format!(
            "… {} more nodes: ask for more with --max\n",
            nodes.len() - max
        ));
    }
    out
}

/// What an agent may do to a node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Press it: a button, a link, a menu item.
    Invoke,
    /// Replace the value of a field.
    Set(String),
    Toggle,
    Expand,
    Collapse,
    Select,
    /// Give it the keyboard focus. The one action that can bring its window to the front.
    Focus,
    /// Scroll it into view.
    Show,
}

impl Action {
    pub fn parse(kind: &str, text: Option<&str>) -> Result<Action, String> {
        match (kind, text) {
            ("set", Some(t)) if t.len() <= MAX_TEXT => Ok(Action::Set(t.to_string())),
            ("set", Some(_)) => Err(format!("text too long: at most {MAX_TEXT} bytes")),
            ("set", None) => Err("set needs the text".into()),
            (_, Some(_)) => Err(format!("{kind} takes no text")),
            ("invoke", None) => Ok(Action::Invoke),
            ("toggle", None) => Ok(Action::Toggle),
            ("expand", None) => Ok(Action::Expand),
            ("collapse", None) => Ok(Action::Collapse),
            ("select", None) => Ok(Action::Select),
            ("focus", None) => Ok(Action::Focus),
            ("show", None) => Ok(Action::Show),
            _ => Err(format!(
                "unknown action {kind}: invoke, set, toggle, expand, collapse, select, focus, show"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(depth: u32, role: &'static str, name: &str, flags: &[&'static str]) -> Node {
        Node {
            depth,
            id: vec![42, depth as i32, 7],
            role,
            name: name.into(),
            flags: flags.to_vec(),
            ..Node::default()
        }
    }

    #[test]
    fn roles_are_named_and_an_unknown_type_is_an_element() {
        assert_eq!(
            (
                role(50000),
                role(50004),
                role(50020),
                role(50030),
                role(50040)
            ),
            ("button", "edit", "text", "document", "appbar")
        );
        assert_eq!(
            (role(50041), role(0), role(-1), role(i32::MIN)),
            ("element", "element", "element", "element")
        );
    }

    #[test]
    fn text_from_an_app_is_one_bounded_line() {
        assert_eq!(clean("  Save\r\n\tas…  now ", 40), "Save as… now");
        assert_eq!(clean("a\u{0}b\u{1b}[31mc", 40), "a b [31mc");
        assert_eq!(clean("abcdefgh", 5), "abcde…");
        assert_eq!(clean("ab cd ef", 5), "ab cd…");
        assert_eq!(clean("", 5), "");
    }

    #[test]
    fn silent_containers_are_left_out_and_their_children_move_up() {
        let nodes = vec![
            node(0, "window", "App", &[]),
            node(1, "pane", "", &[]),
            node(2, "group", "", &[]),
            node(3, "button", "Save", &["invoke"]),
            node(2, "text", "Ready", &[]),
            node(1, "pane", "Sidebar", &[]),
            node(2, "custom", "", &["invoke"]),
            node(1, "button", "Close", &["invoke"]),
        ];
        let kept: Vec<(u32, &str, String)> = prune(nodes)
            .into_iter()
            .map(|n| (n.depth, n.role, n.name))
            .collect();
        assert_eq!(
            kept,
            vec![
                (0, "window", "App".into()),
                (1, "button", "Save".into()),
                (1, "text", "Ready".into()),
                (1, "pane", "Sidebar".into()),
                (2, "custom", String::new()),
                (1, "button", "Close".into())
            ]
        );
    }

    #[test]
    fn a_window_without_a_title_is_still_the_first_line() {
        let kept = prune(vec![
            node(0, "pane", "", &[]),
            node(1, "button", "Go", &["invoke"]),
        ]);
        assert_eq!(
            kept.iter().map(|n| (n.depth, n.role)).collect::<Vec<_>>(),
            vec![(0, "pane"), (1, "button")]
        );
    }

    #[test]
    fn ids_go_out_and_come_back() {
        assert_eq!(
            id_text(&[42, 1_312_340, 4, -2_147_483_646]),
            "42.1312340.4.-2147483646"
        );
        assert_eq!(
            parse_id("42.1312340.4.-2147483646"),
            Some(vec![42, 1_312_340, 4, -2_147_483_646])
        );
        for bad in [
            "",
            "42.",
            ".42",
            "4 2",
            "42.x",
            "99999999999",
            &"1.".repeat(40),
        ] {
            assert_eq!(parse_id(bad), None, "{bad}");
        }
        assert_eq!(parse_id(&vec!["1"; MAX_ID_PARTS + 1].join(".")), None);
    }

    #[test]
    fn the_outline_shows_an_id_only_where_there_is_something_to_do() {
        let mut field = node(1, "edit", "Your \"name\"", &["set", "focused"]);
        field.value = Some("agent".into());
        field.at = Some([16, 110, 176, 22]);
        let nodes = vec![
            node(0, "window", "App", &[]),
            field,
            node(1, "text", "Hello", &[]),
            node(1, "button", "Go", &["invoke", "disabled"]),
        ];
        assert_eq!(
            outline(&nodes, 10, false),
            "window \"App\"\n  edit \"Your 'name'\" [42.1.7] =\"agent\" (set, focused)\n  text \"Hello\"\n  button \"Go\" [42.1.7] (invoke, disabled)\n"
        );
        assert!(outline(&nodes, 10, true).contains("(set, focused) @16,110 176x22\n"));
        assert!(outline(&nodes, 2, false).ends_with("… 2 more nodes: ask for more with --max\n"));
    }

    #[test]
    fn actions_are_a_closed_set() {
        assert_eq!(Action::parse("invoke", None), Ok(Action::Invoke));
        assert_eq!(
            Action::parse("set", Some("")),
            Ok(Action::Set(String::new()))
        );
        assert!(Action::parse("set", None).is_err());
        assert!(Action::parse("set", Some(&"x".repeat(MAX_TEXT + 1))).is_err());
        assert!(Action::parse("invoke", Some("x")).is_err());
        assert!(Action::parse("click", None).is_err());
        for k in ["toggle", "expand", "collapse", "select", "focus", "show"] {
            assert!(Action::parse(k, None).is_ok(), "{k}");
        }
    }
}
