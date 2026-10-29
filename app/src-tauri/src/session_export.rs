//! Markdown export of one past conversation (Sessions panel → Export as Markdown…), read from the
//! agent's own transcript: Claude Code, Codex and OMP, the agents whose transcripts the panel lists.
//!
//! - **Source.** Found by conversation id under the agent's own folder, the way the usage readers
//!   find it (`session_usage::resolve_in`, plus Codex's compressed rollouts); nothing inside a
//!   transcript is ever opened as a path.
//! - **Bounded.** The transcript is streamed line by line: a line over [`MAX_LINE`] is passed over
//!   unread, and the Markdown stops (with a note) once it passes [`MAX_BODY`].
//! - **Redacted.** Every piece of text (header values, messages, tool names, inputs and outputs) goes
//!   through `kelpie_proto::redact` on its own, before it is cut or formatted. Structured tool input
//!   and JSON tool output have each string redacted before they are serialised (JSON escapes would
//!   hide line starts and quotes from the patterns), and the text again after. A tool text is
//!   redacted over what its block keeps plus [`REDACT_MARGIN`], never the whole output.
//! - **Content.** User and assistant messages in full, each under its own heading and kept from
//!   breaking the document (see [`contain`]: a CommonMark parser finds the code, tag-like `<` outside
//!   it are escaped, a message left inside an open fence is written as plain text); tool calls and results as folded `<details>`
//!   blocks, each cut to [`CUT_LINES`] lines / [`CUT_CHARS`] characters with a "… N lines omitted"
//!   marker; thinking left out; images and attachments named (`[image: image/png]`), never embedded.
//!   Line endings are LF, whatever the transcript used. OMP sessions are trees: only the active branch
//!   is exported (see [`active_branch`]).
//! - **Written** beside the chosen file and renamed over it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use kelpie_proto::redact;
use pulldown_cmark::{CodeBlockKind, Event as Md, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_history::{claude_dir, codex_dir, codex_injected, file_id_ok, home_or, is_claude_command, local_home, omp_sessions_dir, open_lines, Agent};
use crate::config_transfer::{path_kind, PathKind};
use crate::fs_ops::{blocking, CmdResult};
use crate::session_usage::{agent_of, find_rollout, find_rollout_by_date, resolve_in};

/// Longest transcript line read (a pasted image or document makes a line large).
const MAX_LINE: usize = 32 << 20;
/// The Markdown stops after the line that takes it past this size.
const MAX_BODY: usize = 16 << 20;
/// What one tool input or result keeps.
const CUT_LINES: usize = 40;
const CUT_CHARS: usize = 4000;
/// The one-line hint in a tool call's summary.
const HINT_CHARS: usize = 80;

/// What the window knows about the conversation (from its Sessions panel row).
pub(crate) struct Request<'a> {
    pub id: &'a str,
    /// Folder the agent recorded; shown when the transcript names none.
    pub cwd: Option<&'a str>,
    pub title: Option<&'a str>,
}

pub(crate) struct Rendered {
    pub text: String,
    /// Lines that were not JSON, were cut off, or were too long to read.
    pub skipped: u32,
    /// The Markdown reached [`MAX_BODY`] before the end of the transcript.
    pub truncated: bool,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Exported {
    path: String,
    skipped: u32,
    truncated: bool,
}

/// One step of a conversation, whatever the agent's format.
#[derive(Debug)]
enum Event {
    User(String),
    Assistant(String),
    Call { id: String, name: String, input: Value },
    /// `name`: when the format repeats it (else the call's, by id).
    Result { id: String, name: Option<String>, output: String, error: bool },
}

#[derive(Default)]
struct Meta {
    cwd: Option<String>,
    started: Option<String>,
    ended: Option<String>,
    models: Vec<String>,
}

impl Meta {
    /// `exported`: the line gave the file something; the end is the last such line's time.
    fn time(&mut self, v: &Value, exported: bool) {
        if let Some(t) = v["timestamp"].as_str().filter(|t| !t.is_empty()) {
            self.started.get_or_insert_with(|| t.to_string());
            if exported {
                self.ended = Some(t.to_string());
            }
        }
    }

    fn cwd(&mut self, cwd: Option<&str>) {
        if self.cwd.is_none() {
            self.cwd = cwd.filter(|c| !c.is_empty()).map(str::to_string);
        }
    }

    /// Each model once, in the order the conversation used them.
    fn model(&mut self, model: Option<&str>) {
        if let Some(m) = model.filter(|m| !m.is_empty()) {
            if !self.models.iter().any(|x| x == m) {
                self.models.push(m.to_string());
            }
        }
    }
}

fn text_of(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// `[image: image/png]`: Claude's `source.media_type`, OMP's `mimeType`, a Codex data URL's type.
fn image_ref(b: &Value) -> String {
    let mime = b["source"]["media_type"]
        .as_str()
        .or(b["mimeType"].as_str())
        .or_else(|| b["image_url"].as_str().and_then(|u| u.strip_prefix("data:")).and_then(|r| r.split([';', ',']).next()));
    match mime.filter(|m| !m.is_empty()) {
        Some(m) => format!("[image: {m}]"),
        None => "[image]".into(),
    }
}

/// `[attachment: application/pdf]` (Claude's document blocks).
fn attachment_ref(b: &Value) -> String {
    match b["source"]["media_type"].as_str() {
        Some(m) => format!("[attachment: {m}]"),
        None => "[attachment]".into(),
    }
}

/// A tool result's text: a string, or blocks (text kept, images named).
fn blocks_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| match b["type"].as_str() {
                Some("text" | "input_text" | "output_text") => b["text"].as_str().map(str::to_string),
                Some("image" | "input_image") => Some(image_ref(b)),
                Some("document") => Some(attachment_ref(b)),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ---------- the three formats ----------

/// Text Claude Code records as a user line that the user did not type (the chat adapter hides the
/// same): markup (`<command-name>`, IDE context), slash-command output, interruption markers.
fn claude_hidden(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('<') || is_claude_command(t)
}

fn claude(v: &Value, meta: &mut Meta, out: &mut Vec<Event>) {
    let kind = v["type"].as_str().unwrap_or_default();
    if kind != "user" && kind != "assistant" {
        return;
    }
    meta.cwd(v["cwd"].as_str());
    // Meta lines, compaction summaries and sub-agent (sidechain) lines are not the conversation.
    if v["isMeta"] == true || v["isCompactSummary"] == true || v["isSidechain"] == true {
        return;
    }
    let msg = &v["message"];
    let user = kind == "user";
    if !user {
        // `<synthetic>`: Claude Code's own notices ("No response requested."), not the model's.
        if msg["model"].as_str().is_some_and(|m| m.starts_with('<')) {
            return;
        }
        meta.model(msg["model"].as_str());
    }
    let said = |text: String| if user { Event::User(text) } else { Event::Assistant(text) };
    match &msg["content"] {
        Value::String(s) if !(user && claude_hidden(s)) => out.push(said(s.clone())),
        Value::Array(blocks) => {
            for b in blocks {
                match b["type"].as_str().unwrap_or_default() {
                    "text" => {
                        let t = text_of(&b["text"]);
                        if !(user && claude_hidden(&t)) {
                            out.push(said(t));
                        }
                    }
                    "image" => out.push(said(image_ref(b))),
                    "document" => out.push(said(attachment_ref(b))),
                    "tool_use" if !user => out.push(Event::Call { id: text_of(&b["id"]), name: text_of(&b["name"]), input: b["input"].clone() }),
                    "tool_result" if user => out.push(Event::Result {
                        id: text_of(&b["tool_use_id"]),
                        name: None,
                        output: blocks_text(&b["content"]),
                        error: b["is_error"] == true,
                    }),
                    // thinking, redacted_thinking, server-side tool blocks
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// A Codex tool output: plain text, `{content, success}`, or content items. Shell results are JSON
/// text `{"output": …, "metadata": {"exit_code": N}}`: the output is shown, a non-zero exit is an error.
fn codex_output(o: &Value) -> (String, bool) {
    match o {
        Value::String(s) => {
            if let Ok(v) = serde_json::from_str::<Value>(s) {
                if let Some(out) = v["output"].as_str() {
                    return (out.to_string(), v["metadata"]["exit_code"].as_i64().is_some_and(|c| c != 0));
                }
            }
            (s.clone(), false)
        }
        Value::Object(_) => (codex_output(&o["content"]).0, o["success"] == false),
        other => (blocks_text(other), false),
    }
}

fn codex(v: &Value, meta: &mut Meta, out: &mut Vec<Event>) {
    let p = &v["payload"];
    match v["type"].as_str().unwrap_or_default() {
        "session_meta" => meta.cwd(p["cwd"].as_str()),
        "turn_context" => {
            meta.cwd(p["cwd"].as_str());
            meta.model(p["model"].as_str());
        }
        // `event_msg` lines repeat these for Codex's own interface; reasoning is thinking.
        "response_item" => match p["type"].as_str().unwrap_or_default() {
            "message" => {
                let role = p["role"].as_str().unwrap_or_default();
                for b in p["content"].as_array().into_iter().flatten() {
                    match (role, b["type"].as_str().unwrap_or_default()) {
                        ("user", "input_text") => {
                            let t = text_of(&b["text"]);
                            if !codex_injected(&t) {
                                out.push(Event::User(t));
                            }
                        }
                        ("user", "input_image") => out.push(Event::User(image_ref(b))),
                        ("assistant", "output_text") => out.push(Event::Assistant(text_of(&b["text"]))),
                        _ => {}
                    }
                }
            }
            "function_call" => {
                let args = &p["arguments"];
                let input = args.as_str().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_else(|| args.clone());
                out.push(Event::Call { id: text_of(&p["call_id"]), name: text_of(&p["name"]), input });
            }
            "custom_tool_call" => out.push(Event::Call { id: text_of(&p["call_id"]), name: text_of(&p["name"]), input: p["input"].clone() }),
            "local_shell_call" => out.push(Event::Call { id: text_of(&p["call_id"]), name: "shell".into(), input: p["action"].clone() }),
            "web_search_call" => out.push(Event::Call { id: String::new(), name: "web_search".into(), input: p["action"].clone() }),
            "function_call_output" | "custom_tool_call_output" => {
                let (output, error) = codex_output(&p["output"]);
                out.push(Event::Result { id: text_of(&p["call_id"]), name: None, output, error });
            }
            _ => {}
        },
        _ => {}
    }
}

fn omp(v: &Value, meta: &mut Meta, out: &mut Vec<Event>) {
    match v["type"].as_str().unwrap_or_default() {
        "session" => meta.cwd(v["cwd"].as_str()),
        "message" => {
            let m = &v["message"];
            match m["role"].as_str().unwrap_or_default() {
                // `synthetic`: injected by OMP itself (auto-continue).
                "user" if m["synthetic"] != true => match &m["content"] {
                    Value::String(s) => out.push(Event::User(s.clone())),
                    Value::Array(blocks) => {
                        for b in blocks {
                            match b["type"].as_str().unwrap_or_default() {
                                "text" => out.push(Event::User(text_of(&b["text"]))),
                                "image" => out.push(Event::User(image_ref(b))),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                },
                "assistant" => {
                    meta.model(m["model"].as_str());
                    for b in m["content"].as_array().into_iter().flatten() {
                        match b["type"].as_str().unwrap_or_default() {
                            "text" => out.push(Event::Assistant(text_of(&b["text"]))),
                            // Narration: prose to the user that the provider returned as a thinking block.
                            "thinking" if b["narration"] == true => out.push(Event::Assistant(text_of(&b["thinking"]))),
                            "image" => out.push(Event::Assistant(image_ref(b))),
                            "toolCall" => out.push(Event::Call { id: text_of(&b["id"]), name: text_of(&b["name"]), input: b["arguments"].clone() }),
                            _ => {}
                        }
                    }
                }
                "toolResult" => out.push(Event::Result {
                    id: text_of(&m["toolCallId"]),
                    name: m["toolName"].as_str().map(str::to_string),
                    output: blocks_text(&m["content"]),
                    error: m["isError"] == true,
                }),
                _ => {}
            }
        }
        _ => {}
    }
}

// ---------- Markdown ----------

/// LF line endings.
fn lf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Longest run of backticks in `s`.
fn ticks(s: &str) -> usize {
    s.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

/// An inline code span that `s` cannot close early.
fn code(s: &str) -> String {
    let fence = "`".repeat(ticks(s) + 1);
    let pad = if s.starts_with('`') || s.ends_with('`') { " " } else { "" };
    format!("{fence}{pad}{s}{pad}{fence}")
}

/// Every string in `v`, keys included, through `redact` on its own: once serialised, JSON escapes
/// (`\n`, `\"`) would hide line starts and quotes from its patterns. Under a secret-named key, at any
/// depth (`{"password": [1234]}`, `{"token": {"value": "x"}}`), every number and string is masked:
/// a bare value has no name next to it for the patterns.
fn redact_value(v: &Value) -> Value {
    redact_under(v, false)
}

fn redact_under(v: &Value, secret: bool) -> Value {
    let mask = || Value::String(kelpie_proto::redact::MASK.into());
    match v {
        Value::String(s) if secret && !s.is_empty() => mask(),
        Value::String(s) => Value::String(redact(s)),
        Value::Number(_) if secret => mask(),
        Value::Array(a) => Value::Array(a.iter().map(|x| redact_under(x, secret)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (redact(k), redact_under(x, secret || secret_name(k)))).collect()),
        other => other.clone(),
    }
}

/// Whether `key` names a secret, by the credential filter's own rule (a quoted value under it is
/// masked). Keys without a secret-like word are not run through the patterns.
fn secret_name(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    ["secret", "token", "pass", "pwd", "key", "credential"].iter().any(|w| k.contains(w))
        && redact(&format!("\"{key}\": \"probe-value-1\"")).contains(kelpie_proto::redact::MASK)
}

/// A JSON object or array given as text (an MCP result, a tool's JSON output), pretty-printed with
/// its strings redacted one by one; `None` for any other text.
fn json_text(text: &str) -> Option<String> {
    let t = text.trim();
    if !(t.starts_with('{') || t.starts_with('[')) {
        return None;
    }
    serde_json::to_string_pretty(&redact_value(&serde_json::from_str::<Value>(t).ok()?)).ok()
}

/// Output with JSON values on lines of their own (JSON lines, `jq -c`, logs): each such line with its
/// strings redacted (kept on one line); `None` when no line is a JSON object or array.
fn json_lines(text: &str) -> Option<String> {
    let mut found = false;
    let out: Vec<String> = text
        .split('\n')
        .map(|line| {
            let t = line.trim();
            let parsed = (t.starts_with('{') || t.starts_with('[')).then(|| serde_json::from_str::<Value>(t).ok()).flatten();
            match parsed.and_then(|v| serde_json::to_string(&redact_value(&v)).ok()) {
                Some(s) => {
                    found = true;
                    s
                }
                None => line.to_string(),
            }
        })
        .collect();
    found.then(|| out.join("\n"))
}

/// `text` with every secret masked that the credential filter finds in it as written or as it reads
/// with JSON string escapes undone (`\n`, `\t`, `\"`, `\\`): JSON that is not whole (after a text
/// prefix, cut, several blocks) hides line starts and quotes from the patterns. The ranges of both
/// readings are merged on the text as written (so source code keeps its `\n`), then the result is
/// filtered once more, which can only mask more.
fn redact_escaped(text: &str) -> String {
    if !text.contains('\\') {
        return redact(text);
    }
    // The unescaped text, and for each of its bytes the bytes of `text` it came from.
    let (mut plain, mut from) = (String::with_capacity(text.len()), Vec::with_capacity(text.len()));
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let unescaped = match (c, chars.peek()) {
            ('\\', Some((_, 'n'))) => Some('\n'),
            ('\\', Some((_, 't'))) => Some('\t'),
            ('\\', Some((_, '"'))) => Some('"'),
            ('\\', Some((_, '\\'))) => Some('\\'),
            _ => None,
        };
        match unescaped {
            Some(u) => {
                chars.next();
                plain.push(u);
                from.push(i..i + 2);
            }
            None => {
                plain.push(c);
                from.extend(std::iter::repeat_n(i..i + c.len_utf8(), c.len_utf8()));
            }
        }
    }
    let mut ranges = kelpie_proto::redact::redact_ranges(text);
    ranges.extend(kelpie_proto::redact::redact_ranges(&plain).into_iter().map(|r| from[r.start].start..from[r.end - 1].end));
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    redact(&kelpie_proto::redact::mask_ranges(text, &merged))
}

/// What a tool block keeps is cut from the start of its text: see [`window`].
const REDACT_MARGIN: usize = 4096;

/// The start of `text` a block can keep plus a margin (at most [`CUT_LINES`] + 1 lines and
/// [`CUT_CHARS`] + [`REDACT_MARGIN`] characters), and how many lines come after it: only this much
/// is redacted, however long the output. A secret that starts in the kept part is masked whole while
/// it is shorter than the margin (token and `NAME=` patterns match a prefix, a key block cut off is
/// masked to its end).
fn window(text: &str) -> (&str, usize) {
    let mut end = text.match_indices('\n').nth(CUT_LINES).map_or(text.len(), |(i, _)| i);
    if let Some((i, _)) = text[..end].char_indices().nth(CUT_CHARS + REDACT_MARGIN) {
        end = i;
    }
    let lines = |s: &str| s.matches('\n').count() + 1;
    (&text[..end], lines(text) - lines(&text[..end]))
}

/// A tool input or output for its block: LF endings, the start redacted, then cut.
fn tool_text(text: &str) -> String {
    let text = lf(text);
    let (head, after) = window(text.trim_end().trim_start_matches('\n'));
    cut(&redact_escaped(head), after)
}

/// The Markdown dialect a viewer renders an export with (the app's `marked`: GitHub tables, task
/// lists, strikethrough).
fn markdown() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH
}

/// What a CommonMark parser makes of a message: the byte ranges of its code (fenced and indented
/// blocks, inline spans), whether any raw HTML is left, and whether a fenced block is still open at
/// the end (it would run on into whatever follows the message).
struct Shape {
    code: Vec<std::ops::Range<usize>>,
    raw_html: bool,
    open_at_end: bool,
}

fn shape(text: &str) -> Shape {
    let mut s = Shape { code: Vec::new(), raw_html: false, open_at_end: false };
    // The open fenced block: its character, and where its content (its last text) ends.
    let mut fence: Option<(char, usize)> = None;
    for (event, r) in Parser::new_ext(text, markdown()).into_offset_iter() {
        match event {
            Md::Start(Tag::CodeBlock(kind)) => {
                if let CodeBlockKind::Fenced(_) = kind {
                    let ch = text[r.clone()].chars().find(|c| *c == '`' || *c == '~').unwrap_or('`');
                    let opener_end = text[r.clone()].find('\n').map_or(r.end, |i| r.start + i + 1);
                    fence = Some((ch, opener_end));
                }
                s.code.push(r);
            }
            Md::Text(_) => {
                if let Some((_, end)) = &mut fence {
                    *end = (*end).max(r.end);
                }
            }
            Md::End(TagEnd::CodeBlock) => {
                // Closed: a run of the fence character follows the content within the block.
                if let Some((ch, content_end)) = fence.take() {
                    let closed = text[content_end.min(r.end)..r.end].contains(&ch.to_string().repeat(3));
                    s.open_at_end |= !closed && text[r.end..].trim().is_empty();
                }
            }
            Md::Code(_) => s.code.push(r),
            Md::Html(_) | Md::InlineHtml(_) => s.raw_html = true,
            _ => {}
        }
    }
    s
}

/// `text` with a `\` before each `<` that starts a tag-like sequence (`<` then a letter, `/`, `!` or
/// `?`) outside `code`, unless a backslash already escapes it.
fn escape_tags(text: &str, code: &[std::ops::Range<usize>]) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 8);
    for (i, c) in text.char_indices() {
        if c == '<'
            && b.get(i + 1).is_some_and(|x| x.is_ascii_alphabetic() || matches!(x, b'/' | b'!' | b'?'))
            && !code.iter().any(|r| r.contains(&i))
            && b[..i].iter().rev().take_while(|&&x| x == b'\\').count().is_multiple_of(2)
        {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Whether `text`, followed by the next message's heading, leaves that heading a heading and has
/// no raw HTML: what the export needs of every message.
fn contained(text: &str) -> bool {
    let doc = format!("{text}\n\n## kelpie-next\n");
    let next = text.len() + 2;
    let mut heading = false;
    for (event, r) in Parser::new_ext(&doc, markdown()).into_offset_iter() {
        match event {
            Md::Html(_) | Md::InlineHtml(_) => return false,
            Md::Start(Tag::Heading { .. }) if r.start == next => heading = true,
            _ => {}
        }
    }
    heading
}

/// `text` as plain text in a fence longer than any backtick or tilde run in it (nothing inside can
/// close it).
fn as_plain(text: &str) -> String {
    let run = |ch: char| text.split(|c| c != ch).map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(run('`').max(run('~')).max(2) + 1);
    format!("{fence}text\n{text}\n{fence}")
}

/// User or assistant text made safe to embed, so one message cannot fold, hide or swallow the rest
/// of the file (a Markdown viewer passes raw HTML through). A CommonMark parser (`pulldown-cmark`,
/// with the viewer's GitHub extensions) says where the code is; every tag-like `<` outside it is
/// escaped. The result is parsed again and must have no raw HTML and leave the next message's
/// heading a heading. A message that ends inside an open code fence (an interrupted reply), or whose
/// result does not pass, is written whole as plain text in an outer fence instead: containment over
/// looks in those rare cases.
fn contain(text: &str) -> String {
    let first = shape(text);
    if !first.open_at_end {
        let mut out = escape_tags(text, &first.code);
        // Escaping can change what parses as code (an HTML block becomes a paragraph): check the
        // result, and escape again by what it now holds.
        for _ in 0..3 {
            let now = shape(&out);
            if now.open_at_end {
                break;
            }
            if !now.raw_html && contained(&out) {
                return out;
            }
            out = escape_tags(&out, &now.code);
        }
    }
    as_plain(text)
}

/// At most [`CUT_LINES`] lines and [`CUT_CHARS`] characters of `text`, then "… N lines omitted"
/// (`after`: lines that followed `text` in the original). A first line longer than the whole
/// allowance keeps its start, marked with `…`.
fn cut(text: &str, after: usize) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let (mut out, mut chars, mut used) = (String::new(), 0, 0);
    for line in &lines {
        if used == CUT_LINES {
            break;
        }
        let n = line.chars().count();
        if chars + n > CUT_CHARS {
            if used == 0 {
                out.extend(line.chars().take(CUT_CHARS));
                out.push('…');
                used = 1;
            }
            break;
        }
        if used > 0 {
            out.push('\n');
        }
        out.push_str(line);
        chars += n + 1;
        used += 1;
    }
    match lines.len() - used + after {
        0 => {}
        1 => out.push_str("\n… 1 line omitted"),
        n => out.push_str(&format!("\n… {n} lines omitted")),
    }
    out
}

/// What a tool call is about, for its summary line: the command, file, pattern, URL…
fn hint(input: &Value) -> Option<String> {
    let pick = ["command", "cmd", "file_path", "path", "pattern", "url", "query", "description", "prompt"].iter().find_map(|k| match &input[*k] {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => Some(a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
        _ => None,
    })?;
    let line = one_line(&redact(&pick));
    if line.is_empty() {
        return None;
    }
    Some(if line.chars().count() > HINT_CHARS { line.chars().take(HINT_CHARS).collect::<String>() + "…" } else { line })
}

#[derive(Clone, Copy, PartialEq)]
enum Role {
    User,
    Assistant,
}

#[derive(Default)]
struct Doc {
    body: String,
    role: Option<Role>,
    /// Tool names by call id (results in Claude and Codex name only the call).
    names: HashMap<String, String>,
    /// Text of the transcript entry being read, joined until something else comes: one message.
    pending: Option<(Role, String)>,
}

impl Doc {
    fn heading(&mut self, role: Role) {
        if self.role != Some(role) {
            self.role = Some(role);
            self.body.push_str(if role == Role::User { "## User\n\n" } else { "## Assistant\n\n" });
        }
    }

    fn push(&mut self, e: Event) {
        let (role, text) = match e {
            Event::User(t) => (Role::User, t),
            Event::Assistant(t) => (Role::Assistant, t),
            other => {
                self.end_message();
                return self.tool(other);
            }
        };
        match &mut self.pending {
            Some((r, buf)) if *r == role => {
                buf.push_str("\n\n");
                buf.push_str(&text);
            }
            _ => {
                self.end_message();
                self.pending = Some((role, text));
            }
        }
    }

    /// Writes the message being gathered, if any (at the end of each transcript entry).
    fn end_message(&mut self) {
        if let Some((role, text)) = self.pending.take() {
            self.message(role, &text);
        }
    }

    fn tool(&mut self, e: Event) {
        match e {
            Event::User(_) | Event::Assistant(_) => {}
            Event::Call { id, name, input } => {
                if !id.is_empty() {
                    self.names.insert(id, name.clone());
                }
                // Strings are redacted before serialising (and the text again after, as a second net).
                let (text, lang) = match &input {
                    Value::String(s) => json_text(s).map(|j| (j, "json")).unwrap_or_else(|| (json_lines(s).unwrap_or_else(|| s.clone()), "text")),
                    Value::Null => (String::new(), "text"),
                    v => (serde_json::to_string_pretty(&redact_value(v)).unwrap_or_default(), "json"),
                };
                let hint = hint(&input).map(|h| format!(" · <code>{}</code>", html(&h))).unwrap_or_default();
                self.details(&format!("Tool call: {}{hint}", html(&one_line(&redact(&name)))), &text, lang);
            }
            Event::Result { id, name, output, error } => {
                let name = name.or_else(|| self.names.get(&id).cloned()).map(|n| format!(": {}", html(&one_line(&redact(&n))))).unwrap_or_default();
                let (text, lang) = match json_text(&output) {
                    Some(j) => (j, "json"),
                    None => (json_lines(&output).unwrap_or(output), "text"),
                };
                self.details(&format!("Result{name}{}", if error { " (error)" } else { "" }), &text, lang);
            }
        }
    }

    fn message(&mut self, role: Role, text: &str) {
        let text = redact(&lf(text));
        let text = text.trim_end().trim_start_matches('\n');
        if text.trim().is_empty() {
            return;
        }
        // Every message under a heading of its own: a list or code block cannot run on from the one
        // before.
        self.role = Some(role);
        self.body.push_str(if role == Role::User { "## User\n\n" } else { "## Assistant\n\n" });
        self.body.push_str(&contain(text));
        self.body.push_str("\n\n");
    }

    /// A folded block: `summary` is HTML already; `text` is redacted, cut and fenced here.
    fn details(&mut self, summary: &str, text: &str, lang: &str) {
        // Tool steps belong to the assistant's turn (Claude records results as user lines).
        self.heading(Role::Assistant);
        let text = tool_text(text);
        self.body.push_str("<details>\n<summary>");
        self.body.push_str(summary);
        self.body.push_str("</summary>\n\n");
        if text.trim().is_empty() {
            self.body.push_str("*(empty)*\n\n");
        } else {
            let fence = "`".repeat(ticks(&text).max(2) + 1);
            self.body.push_str(&format!("{fence}{lang}\n{text}\n{fence}\n\n"));
        }
        self.body.push_str("</details>\n\n");
    }
}

/// `16 MiB`, `2000 bytes`.
fn size(bytes: usize) -> String {
    if bytes >= 1 << 20 && bytes.is_multiple_of(1 << 20) { format!("{} MiB", bytes >> 20) } else { format!("{bytes} bytes") }
}

fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "Claude",
        Agent::Codex => "Codex",
        Agent::Omp => "OMP",
    }
}

fn header(agent: Agent, req: &Request, meta: &Meta) -> String {
    let title = req.title.map(|t| one_line(&redact(t))).filter(|t| !t.is_empty()).unwrap_or_else(|| "Conversation".into());
    let mut rows = vec![("Agent", agent_name(agent).to_string())];
    if let Some(folder) = meta.cwd.as_deref().or(req.cwd).filter(|c| !c.is_empty()) {
        rows.push(("Project folder", code(&one_line(&redact(folder)))));
    }
    rows.push(("Session id", code(&redact(req.id))));
    if let Some(t) = &meta.started {
        rows.push(("Started", one_line(&redact(t))));
    }
    if let Some(t) = &meta.ended {
        rows.push(("Ended", one_line(&redact(t))));
    }
    if !meta.models.is_empty() {
        let label = if meta.models.len() == 1 { "Model" } else { "Models" };
        rows.push((label, one_line(&meta.models.iter().map(|m| redact(m)).collect::<Vec<_>>().join(", "))));
    }
    let mut out = format!("# {title}\n\n");
    for (label, value) in rows {
        out.push_str(&format!("- **{label}:** {value}\n"));
    }
    out.push_str("\n---\n\n");
    out
}

/// The parts of an OMP entry that place it in the session tree.
#[derive(Deserialize)]
struct Link {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<String>,
    #[serde(rename = "parentId")]
    parent: Option<String>,
    /// Set on an entry appended to another branch: the leaf stays where it was.
    #[serde(rename = "activeLeafId")]
    active_leaf: Option<String>,
}

/// The entries of an OMP session's active branch: from the leaf (the last entry, or the leaf an
/// off-branch entry names) back to the root through `parentId`. Branches left by `/tree` or a fork
/// are not part of what happened. `None` (exported in file order) for a session without parent links,
/// or when the chain reaches an entry that could not be read.
///
/// Claude Code also links lines (`parentUuid`), but its chain also forks for parallel tool results
/// and progress lines, which a walk from the leaf would drop; Kelpie's chat view replays Claude
/// history in file order too, so Claude is exported in file order. Codex rollouts are linear.
fn active_branch(mut next: impl FnMut() -> Option<Result<String, u64>>) -> Option<HashSet<String>> {
    let (mut parents, mut leaf, mut linked, mut session) = (HashMap::<String, Option<String>>::new(), None, false, None);
    while let Some(line) = next() {
        let Some(link) = line.ok().and_then(|l| serde_json::from_str::<Link>(&l).ok()) else { continue };
        if link.kind.as_deref() == Some("session") {
            session = link.id;
            continue;
        }
        let Some(id) = link.id else { continue };
        linked |= link.parent.is_some();
        leaf = Some(link.active_leaf.unwrap_or_else(|| id.clone()));
        parents.insert(id, link.parent);
    }
    let mut at = leaf.filter(|_| linked)?;
    let mut path = HashSet::new();
    while path.insert(at.clone()) {
        match parents.get(&at) {
            // The root's parent is none, or the session header itself.
            Some(Some(up)) if session.as_ref() == Some(up) => break,
            Some(Some(up)) => at = up.clone(),
            Some(None) => break,
            // A link to an entry that could not be read (cut, too long, malformed): which entries
            // came before it is unknown, so everything is exported in file order instead.
            None => return None,
        }
    }
    Some(path)
}

/// The Markdown of a transcript given as lines (`Err(len)`: a line too long to read), stopping after
/// the line that takes it past `max_body` bytes. `keep`: the entry ids to export (lines without an
/// id are always read).
fn render_lines(agent: Agent, mut next: impl FnMut() -> Option<Result<String, u64>>, req: &Request, max_body: usize, keep: Option<&HashSet<String>>) -> Rendered {
    let (mut meta, mut doc, mut events) = (Meta::default(), Doc::default(), Vec::new());
    let (mut skipped, mut truncated) = (0u32, false);
    while let Some(line) = next() {
        if doc.body.len() >= max_body {
            truncated = true;
            break;
        }
        let Ok(line) = line else {
            skipped += 1;
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            skipped += 1;
            continue;
        };
        if let (Some(keep), Some(id)) = (keep, v["id"].as_str()) {
            if v["type"] != "session" && !keep.contains(id) {
                continue;
            }
        }
        match agent {
            Agent::Claude => claude(&v, &mut meta, &mut events),
            Agent::Codex => codex(&v, &mut meta, &mut events),
            Agent::Omp => omp(&v, &mut meta, &mut events),
        }
        meta.time(&v, !events.is_empty());
        for e in events.drain(..) {
            doc.push(e);
        }
        doc.end_message();
    }
    doc.end_message();
    let mut text = header(agent, req, &meta);
    let body = doc.body.trim_end();
    text.push_str(if body.is_empty() { "*No messages.*" } else { body });
    text.push('\n');
    let mut notes = Vec::new();
    if truncated {
        notes.push(format!("*The export stops here: the conversation is longer than {} of Markdown.*", size(max_body)));
    }
    match skipped {
        0 => {}
        1 => notes.push("*1 transcript line could not be read and is left out.*".into()),
        n => notes.push(format!("*{n} transcript lines could not be read and are left out.*")),
    }
    if !notes.is_empty() {
        text.push_str(&format!("\n---\n\n{}\n", notes.join("\n\n")));
    }
    Rendered { text, skipped, truncated }
}

/// The Markdown of the transcript at `path` (plain or zstd-compressed JSON lines).
pub(crate) fn render(agent: Agent, path: &Path, req: &Request) -> CmdResult<Rendered> {
    let open = || open_lines(path, 0, u64::MAX).ok_or("the transcript could not be opened");
    let keep = if agent == Agent::Omp {
        let mut lines = open()?;
        active_branch(|| lines.next_capped(MAX_LINE))
    } else {
        None
    };
    let mut lines = open()?;
    Ok(render_lines(agent, || lines.next_capped(MAX_LINE), req, MAX_BODY, keep.as_ref()))
}

/// The transcript of conversation `id`, only under the agent's own folder (`home`: an account's
/// config home, for Claude and Codex).
fn locate(agent: Agent, id: &str, cwd: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    if !file_id_ok(id) {
        return None;
    }
    let root = match agent {
        Agent::Claude => home_or(home, claude_dir),
        Agent::Codex => home_or(home, codex_dir),
        Agent::Omp => omp_sessions_dir(),
    };
    resolve_in(agent, &root, id, None, cwd).or_else(|| {
        // Codex compresses older rollouts.
        let suffix = format!("-{id}.jsonl.zst");
        (agent == Agent::Codex).then(|| find_rollout_by_date(&root, id, &suffix).or_else(|| find_rollout(&root.join("sessions"), &suffix, 0)))?
    })
}

fn export_to(kind: &str, id: &str, cwd: Option<&str>, home: Option<&str>, title: Option<&str>, path: &str) -> CmdResult<Exported> {
    let agent = agent_of(kind).ok_or("this agent's conversations cannot be exported")?;
    // Classified by its text before anything opens it: even reading a device path's metadata opens
    // it. A network share chosen in the dialog is the user's choice and allowed.
    if path_kind(path) == PathKind::Device {
        return Err("not a file".into());
    }
    let target = Path::new(path);
    if !target.is_absolute() {
        return Err("choose a file in a folder".into());
    }
    if !file_id_ok(id) {
        return Err("invalid session id".into());
    }
    if !local_home(home) {
        return Err("the account folder is on another computer".into());
    }
    let source = locate(agent, id, cwd, home).ok_or("the transcript of this conversation was not found")?;
    if target.canonicalize().ok().is_some_and(|t| source.canonicalize().ok() == Some(t)) {
        return Err("choose a file other than the transcript".into());
    }
    let doc = render(agent, &source, &Request { id, cwd, title })?;
    crate::config_transfer::write_file_atomic(target, doc.text.as_bytes())?;
    Ok(Exported { path: path.to_string(), skipped: doc.skipped, truncated: doc.truncated })
}

/// Writes conversation `id` of `agent` as Markdown to `path` (from the save dialog).
#[tauri::command]
pub async fn agent_session_export(agent: String, id: String, cwd: Option<String>, home: Option<String>, title: Option<String>, path: String) -> CmdResult<Exported> {
    blocking(move || export_to(&agent, &id, cwd.as_deref(), home.as_deref(), title.as_deref(), &path)).await
}

#[cfg(test)]
mod tests;
