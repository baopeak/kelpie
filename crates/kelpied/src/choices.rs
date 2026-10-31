//! Prompts an agent shows on screen and waits on: numbered menus, yes/no questions. Agents
//! without hooks (Codex approvals, Gemini, Aider, OpenCode…) only say they need the user this way.
//!
//! Only the lines at the cursor count — the last non-empty lines of the screen — so a shell that
//! printed a file mentioning "(y/n)" and went back to its prompt is not waiting.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Prompt {
    /// Keys that answer it, in screen order: digits for a menu, letters for yes/no.
    pub keys: Vec<String>,
    /// The question line above the choices, when there is one.
    pub question: Option<String>,
    /// Answered by typing a line (`Read-Host`, `[Y/n]`): the key needs Enter after it. Menus and
    /// single-key approvals act on the key alone.
    pub line: bool,
    /// Keys whose choice outlasts this prompt ("don't ask again", "always", "for this session"): for the computer only, a
    /// phone's approvals are one-shot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lasting: Vec<String>,
    /// The key the menu's marker is on: what Enter would choose. The computer's business, not sent to phones.
    #[serde(skip)]
    pub selected: Option<String>,
}

impl Prompt {
    /// Whether an answer from a phone (a named key, or typed text) can only be one of this prompt's plain choices. While something on
    /// it lasts, that means `esc`, exactly a listed key that does not last, or Enter on a marker known to be on one: typed text
    /// goes in as it is, so `2x`, `yes` or a `2` with an invisible character after it would choose what the button is hidden for.
    pub fn plain_answer(&self, key: Option<&str>, text: Option<&str>) -> bool {
        if self.lasting.is_empty() || key == Some("esc") {
            return true;
        }
        let chosen = key.or(text).unwrap_or_default();
        if chosen.is_empty() || key == Some("enter") {
            return self.selected.as_ref().is_some_and(|s| !self.lasting.contains(s));
        }
        self.keys.iter().any(|k| k == chosen) && !self.lasting.iter().any(|k| k == chosen)
    }
}

/// How far up a menu may start, in non-empty lines from the bottom.
const MENU_LINES: usize = 12;
/// A yes/no prompt must sit on one of the last lines.
const YES_NO_LINES: usize = 3;

/// `❯ 1. Yes`, `● 2) No`, `  3. No, and tell Claude what to do differently (esc)`.
static MENU_ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:[│|]\s*)?(?:[❯>›»▶●○◉◯*]\s*)?(\d)[.)]\s+(\S.*)$").unwrap());
/// `(y/n)`, `[Y/n]`, `(yes/no)`.
static YES_NO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)[(\[]\s*(?:y|yes)\s*/\s*(?:n|no)\s*[)\]]").unwrap());
/// Aider: `(Y)es/(N)o/(D)on't ask again`.
static PAREN_INITIALS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(([A-Za-z])\)[a-z']+").unwrap());
/// Codex: `Yes (y)`, `No, provide feedback (n)`, `Always (a)`.
static TRAILING_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(yes|no|always|allow|deny|approve|reject|cancel)\b[^()\n]{0,40}\(([a-z])\)").unwrap());
/// PowerShell's confirmation: `[Y] Yes  [A] Yes to All  [N] No  [L] No to All  [?] Help`.
static BRACKET_KEYS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([A-Za-z?])\]\s+[A-Z]").unwrap());
/// `git add -p`: `(1/2) Stage this hunk [y,n,q,a,d,e,?]? `.
static KEY_LIST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[((?:[a-z?],)+[a-z?])\]\?\s*$").unwrap());
/// A line where the user types: a shell prompt (PowerShell, cmd, sh), or an agent's input box.
static INPUT_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:PS [^>]*>.*|[A-Za-z]:\\[^>]*>.*|[\w@.~:/\\\-\[\]() ]*[$#%]\s.*|[\w@.~:/\\\-\[\]() ]*[$#%]|\s*[>❯›▌]\s*(?:[^0-9\s].*)?)$").unwrap()
});

/// The prompt at the bottom of `screen` (plain text, as `text_tail` returns it), if any.
#[cfg(test)]
pub fn detect(screen: &str) -> Option<Prompt> {
    detect_with_screen(screen).map(|(p, _)| p)
}

/// The prompt and the screen up to its last line: what identifies this prompt among others
/// alike (the command a permission menu is about sits above it). Lines below it — output that
/// followed an answer — are not part of it.
pub fn detect_with_screen(screen: &str) -> Option<(Prompt, String)> {
    let lines: Vec<&str> = screen.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    let (mut prompt, end) = menu(&lines).or_else(|| yes_no(&lines).map(|p| (p, lines.len() - 1)))?;
    prompt.lasting = lasting_keys(&lines, end, &prompt.keys);
    Some((prompt, lines[..=end].join("\n")))
}

// What follows recognises real labels so a phone offers no button for a choice that lasts. It is a convenience on what a person sees, not
// a boundary: a phone allowed to type can reach any choice with the arrow keys, which is accepted and documented. Recognition is best
// effort. The table of real labels in the tests (`real_labels`) is what to hold a change of these rules against.

/// Words that say a choice outlasts the prompt, wherever they are.
static STRONG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:don['’]?t|do not) (?:ask|prompt)\b|(?:don['’]?t|do not) (?:show|remind)(?: me)?(?: .*)? again|\balways\b|\bin the future\b|\bremember\b|\bfrom now on\b|\bauto[- ]?(?:accept|approve|allow)|\b(?:use|set) auto mode\b|\bbypass|\byolo\b|\bforever\b|\bpermanently\b|\bwithout (?:asking|prompting|confirm)").unwrap()
});
/// Words that say so only on a choice that starts by agreeing or refusing, or on a question of permission: `Yes, for this session` and
/// `Yes, for this project` last, and `Block` answering `Which style?`, or `for this project` in a question or a description, do not.
static WEAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bfuture\b|\bfor (?:this|the) (?:project|repo|repository|folder|directory|workspace)\b|\bsessions?\b|\bblock\b|\bnever\b|\brest of (?:the|this)\b").unwrap());
/// A word that starts a choice by refusing.
static REFUSAL_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?:no|n|deny|reject|decline|cancel|abort|exit|stop|skip|quit|disable|don['’]?t|do not)\b").unwrap());
/// A choice that says yes to this one prompt: a word of consent and nothing more, or `once` / `this time`.
static ONCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:yes|y|ok|okay|allow|approve|accept|proceed|continue|confirm|enable|grant|use|turn on|run(?: it)?)(?:[,:]?\s*(?:yes|proceed|continue|run it|but ask again next time|(?:allow |approve |accept |run )?(?:just )?(?:once|this time|this once)))?[.!]?(?:\s*\((?:[a-z]|esc|enter|tab|shift\+tab)\))?$").unwrap()
});
/// A choice that says no, or that only says something back. What follows the word and a comma or a dash (`No, and tell Claude what to
/// do differently`) grants nothing, and `STRONG` is asked first; `No more prompts` is another matter.
static REFUSES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:(?:no|n|deny|reject|decline|cancel|abort|exit|stop|skip|quit|don['’]?t|do not)(?:\s*$|\s*[,.(—–-])|type (?:something|here)\b|other\b)").unwrap()
});
/// A word of consent starting a choice. On its own it says nothing: `Yes` answers any question.
static CONSENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?:yes|allow|approve|accept|always|trust|proceed|run|continue|confirm|enable|grant|use|turn on)\b").unwrap());
/// A question that names permission: that is enough, whatever its choices are called.
static NAMES_PERMISSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:allow|allowed|permission|permissions|permit|grant|trust)\b").unwrap());
/// Going on with what is under way: with a word of consent among the choices, a question of permission.
static GOES_ON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:proceed|continue|execute|approve)\b").unwrap());
/// What an agent may do or may be about to: with a word of consent, a question of permission, unless the choices come with
/// descriptions, which is the form of an agent asking the person what they would like (`Should I create a PR?`).
static ACTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:run|edit|write|delete|remove|create|overwrite|install)\b").unwrap());
/// What makes the yes of a question last, besides the words in `STRONG`: trust is kept.
static TRUST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\btrust\b").unwrap());
/// A letter at the end of a numbered choice, which types the same choice (`2. Yes, and don't ask again (a)`).
static ITEM_LETTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(([A-Za-z])\)[^()]*$").unwrap());
/// A tool's line above the choices (`● Bash(npm test)`, `Bash command`, `$ cargo build`).
static TOOL_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\W*(?:(?:Bash|Edit|MultiEdit|Write|NotebookEdit|Read|Update|Create|WebFetch|WebSearch|Task)(?:\(| command\b| file\b)|\$ )").unwrap());
/// A choice that speaks of permission.
static PERMISSION_CHOICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:allow|approve|grant|permission|permissions|trust)\b").unwrap());
/// A choice that starts by going on or running something.
static GOES_ON_CHOICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?:proceed|run|continue|confirm|accept)\b").unwrap());

/// Whether a menu asks for permission. A question that names it does on its own. Otherwise it takes a word of consent among the
/// choices and something that says so: a tool's line above, or a choice that speaks of it. `Yes` alone does not (`Ship it?`), and an
/// action or going on does not when the choices carry descriptions, the form of an agent asking what the person would like
/// (`Should I create a PR?`, `How should I proceed?`).
fn asks_permission(above: &[&str], menu: &[MenuItem]) -> bool {
    let question = || above.iter().rev().take(3);
    if question().any(|l| NAMES_PERMISSION.is_match(l)) {
        return true;
    }
    if !menu.iter().any(|item| CONSENT.is_match(&item.label)) {
        return false;
    }
    let described = menu.iter().any(|item| item.first_line != item.label);
    above.iter().rev().take(6).any(|l| TOOL_LINE.is_match(l))
        || menu.iter().any(|item| PERMISSION_CHOICE.is_match(&item.label))
        || (!described && (question().any(|l| GOES_ON.is_match(l) || ACTION.is_match(l)) || menu.iter().any(|item| GOES_ON_CHOICE.is_match(&item.label))))
}

/// Whether the lines above a prompt make its yes last: a question of trust anywhere above, or a strong word in the question or the line
/// right over it (`Always allow this tool?`, `Remember this for all projects.`). Plain output above a question says nothing else about it.
fn question_lasts(above: &[&str]) -> bool {
    if above.iter().any(|l| TRUST.is_match(l)) {
        return true;
    }
    let question = above.iter().enumerate().rev().take(3).find(|(_, l)| l.trim_end().ends_with('?')).map(|(i, _)| i).or(above.len().checked_sub(1));
    question.is_some_and(|q| above[q.saturating_sub(1)..=q].iter().any(|l| STRONG.is_match(l)))
}

/// What a choice's label says about how long it holds. Beyond plain once and plain no it cannot be told, and on a question of
/// permission what cannot be told counts as lasting: a phone's approvals are one-shot, and an agent's wording changes.
fn outlasts(label: &str, permission: bool) -> bool {
    let label = label.trim();
    STRONG.is_match(label) || (WEAK.is_match(label) && (permission || opens(label))) || (permission && !ONCE.is_match(label) && !REFUSES.is_match(label))
}

/// Whether a choice starts by agreeing or refusing.
fn opens(label: &str) -> bool {
    CONSENT.is_match(label) || ONCE.is_match(label) || REFUSAL_WORD.is_match(label)
}

/// `outlasts` for a numbered choice. What is under its first line may be a description: only a strong word counts there, unless the
/// question is one of permission, and its first line alone can be the plain answer.
fn item_outlasts(item: &MenuItem, permission: bool) -> bool {
    let first = item.first_line.trim();
    STRONG.is_match(&item.label) || (permission && WEAK.is_match(&item.label)) || (WEAK.is_match(first) && opens(first)) || (permission && !ONCE.is_match(first) && !REFUSES.is_match(first))
}

/// One choice of a numbered menu.
struct MenuItem {
    key: String,
    /// All its text: the lines it wrapped onto, and the description under it.
    label: String,
    /// Its text without a description. A line starting in lowercase goes on the sentence above it, so it is a wrap, not a description.
    first_line: String,
}

/// Adds a line under a menu item being read: (key, label, first line, column).
fn add_line(item: &mut (String, String, String, usize), line: &str) {
    let text = line.trim().trim_matches(['│', '|', ' ']);
    let wraps = item.1 == item.2 && text.chars().next().is_some_and(char::is_lowercase);
    item.1.push(' ');
    item.1.push_str(text);
    if wraps {
        item.2 = item.1.clone();
    }
}

/// The numbered menu that ends the prompt as `(key, label)`, an item's label with the lines it wrapped onto, and where it starts
/// in `lines`. Between two items everything is the upper one's; below the last only what is indented like its text (a hint line
/// such as `Esc to cancel` is not).
fn menu_labels(lines: &[&str], end: usize) -> (Vec<MenuItem>, usize) {
    let start = lines.len().saturating_sub(MENU_LINES).min(end);
    let mut items: Vec<(String, String, String, usize)> = Vec::new();
    let mut first = start;
    for (i, line) in lines.iter().enumerate().take(end + 1).skip(start) {
        match MENU_ITEM.captures(line) {
            Some(c) => {
                if &c[1] == "1" {
                    items.clear();
                    first = i;
                }
                let column = line[..c.get(2).expect("the label").start()].chars().count();
                items.push((c[1].to_string(), c[2].to_string(), c[2].to_string(), column));
            }
            None => {
                if let Some(item) = items.last_mut() {
                    add_line(item, line);
                }
            }
        }
    }
    if let Some(last) = items.last_mut() {
        for line in &lines[end + 1..] {
            let indent = line.chars().take_while(|c| c.is_whitespace() || matches!(c, '│' | '|')).count();
            if indent < last.3 {
                break;
            }
            add_line(last, line);
        }
    }
    let clean = |text: &str| text.trim_end_matches(['│', '|', ' ']).to_string();
    (items.into_iter().map(|(key, label, first_line, _)| MenuItem { key, label: clean(&label), first_line: clean(&first_line) }).collect(), first)
}

/// Which of `keys` choose something that lasts, from the lines of the prompt (`end` is its last line): a numbered item, `Always (a)`,
/// `(D)on't ask again`, `[A] Yes to All`, or a key with no label beyond yes and no.
fn lasting_keys(lines: &[&str], end: usize, keys: &[String]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let (menu, first) = menu_labels(lines, end);
    if !menu.is_empty() && keys.iter().all(|k| menu.iter().any(|item| &item.key == k)) {
        let above = &lines[lines.len().saturating_sub(MENU_LINES).min(first)..first];
        let permission = asks_permission(above, &menu);
        let yes_lasts = question_lasts(above);
        for item in &menu {
            if item_outlasts(item, permission) || (yes_lasts && (CONSENT.is_match(&item.label) || ONCE.is_match(item.first_line.trim()))) {
                found.push(item.key.clone());
                // The same choice by its letter, where the menu gives one.
                if let Some(letter) = ITEM_LETTER.captures(&item.label) {
                    found.push(letter[1].to_string());
                }
            }
        }
        return dedup(found);
    }
    let tail = &lines[lines.len().saturating_sub(MENU_LINES)..];
    for line in tail {
        if MENU_ITEM.is_match(line) {
            continue;
        }
        for c in TRAILING_KEY.captures_iter(line) {
            if outlasts(&c[0], true) {
                // As the prompt lists it: where it tells `a` from `A`, they are two choices.
                found.push(if keys.iter().any(|k| k == &c[2]) { c[2].to_string() } else { c[2].to_lowercase() });
            }
        }
        // `(Y)es/(N)o/(D)on't ask again`, and `[Y] Yes  [A] Yes to All  [N] No`: from one key to the next is its label.
        let initials: Vec<_> = PAREN_INITIALS.captures_iter(line).collect();
        for (i, c) in initials.iter().enumerate() {
            let m = c.get(0).expect("the whole match");
            let end = initials.get(i + 1).map_or(line.len(), |n| n.get(0).expect("the whole match").start());
            let label = line[m.start()..end].replace(['(', ')'], "");
            if outlasts(label.split(['/', '[']).next().unwrap_or_default(), true) {
                found.push(c[1].to_lowercase());
            }
        }
        let bracketed: Vec<_> = BRACKET_KEYS.captures_iter(line).collect();
        for (i, c) in bracketed.iter().enumerate() {
            let m = c.get(0).expect("the whole match");
            let start = m.start() + 3;
            let end = bracketed.get(i + 1).map_or(line.len(), |n| n.get(0).expect("the whole match").start());
            if outlasts(&line[start..end], true) {
                found.push(c[1].to_lowercase());
            }
        }
        // `[y,n,q,a,d,e,?]`: nothing says what they do, and only yes and no are known.
        if let Some(c) = KEY_LIST.captures(line) {
            found.extend(c[1].split(',').filter(|k| !matches!(*k, "y" | "n")).map(str::to_string));
        }
    }
    // `Always allow this command? (y/n)`, or a line above saying what the yes keeps: the yes says what the question says.
    if let Some(m) = tail.last().and_then(|line| YES_NO.find(line)) {
        let on_its_line = tail[tail.len() - 1][..m.start()].trim();
        let mut above = tail[..tail.len() - 1].to_vec();
        if !on_its_line.is_empty() {
            above.push(on_its_line);
        }
        if question_lasts(&above) {
            found.push("y".to_string());
        }
    }
    dedup(found.into_iter().filter(|k| keys.contains(k)).collect())
}

/// The menu and the index of its last item in `lines`.
fn menu(lines: &[&str]) -> Option<(Prompt, usize)> {
    let start = lines.len().saturating_sub(MENU_LINES);
    let window = &lines[start..];
    // The menu's items, bottom-most run of consecutive numbers from 1.
    let numbered: Vec<(usize, u32)> = window.iter().enumerate().filter_map(|(i, l)| MENU_ITEM.captures(l).map(|c| (i, c[1].parse().unwrap_or(0)))).collect();
    let first = numbered.iter().rposition(|&(_, n)| n == 1)?;
    let items: Vec<(usize, u32)> = numbered[first..].iter().copied().enumerate().take_while(|&(k, (_, n))| n == k as u32 + 1).map(|(_, x)| x).collect();
    if items.len() < 2 {
        return None;
    }
    // Waiting on it: the menu ends near the bottom (a hint line such as "Esc to cancel" may follow)
    // and something marks it as a choice — a selection marker or a question just above it.
    let last_item = items.last()?.0;
    if window.len() - 1 - last_item > 3 {
        return None;
    }
    // Something typed below it (a shell prompt, an agent's input box): output, not a prompt.
    if window[last_item + 1..].iter().any(|l| INPUT_LINE.is_match(l)) {
        return None;
    }
    let marked = items.iter().any(|&(i, _)| window[i].trim_start().trim_start_matches(['│', '|']).trim_start().starts_with(['❯', '>', '›', '»', '▶', '●', '◉']));
    let question = window[..items[0].0].iter().rev().take(3).find(|l| l.trim_end().ends_with('?')).map(|l| clean(l));
    // Without a selection marker, only a list that ends the screen asks something.
    if !marked && (question.is_none() || last_item + 1 < window.len()) {
        return None;
    }
    let selected = items.iter().find(|&&(i, _)| window[i].trim_start().trim_start_matches(['│', '|']).trim_start().starts_with(['❯', '>', '›', '»', '▶', '●', '◉'])).map(|(_, n)| n.to_string());
    Some((Prompt { keys: items.iter().map(|(_, n)| n.to_string()).collect(), question, line: false, lasting: Vec::new(), selected }, start + last_item))
}

fn yes_no(lines: &[&str]) -> Option<Prompt> {
    let tail = &lines[lines.len().saturating_sub(YES_NO_LINES)..];
    let last = *tail.last()?;
    if YES_NO.is_match(last) {
        return Some(Prompt { keys: vec!["y".into(), "n".into()], question: Some(clean(last)), line: true, lasting: Vec::new(), selected: None });
    }
    let bracket: Vec<String> = BRACKET_KEYS.captures_iter(last).map(|c| c[1].to_lowercase()).collect();
    if bracket.len() >= 2 && bracket.contains(&"y".to_string()) && bracket.contains(&"n".to_string()) {
        return Some(Prompt { keys: dedup(bracket), question: Some(clean(last)), line: true, lasting: Vec::new(), selected: None });
    }
    if let Some(c) = KEY_LIST.captures(last) {
        let keys: Vec<String> = c[1].split(',').map(str::to_string).collect();
        if keys.contains(&"y".to_string()) && keys.contains(&"n".to_string()) {
            return Some(Prompt { keys, question: Some(clean(last)), line: true, lasting: Vec::new(), selected: None });
        }
    }
    let initials: Vec<String> = PAREN_INITIALS.captures_iter(last).map(|c| c[1].to_lowercase()).collect();
    if initials.len() >= 2 && initials.contains(&"y".to_string()) && initials.contains(&"n".to_string()) {
        return Some(Prompt { keys: dedup(initials), question: Some(clean(last)), line: true, lasting: Vec::new(), selected: None });
    }
    let found: Vec<(String, String)> = tail.iter().flat_map(|l| TRAILING_KEY.captures_iter(l).map(|c| (c[1].to_lowercase(), c[2].to_string())).collect::<Vec<_>>()).collect();
    // Lowercase, unless the prompt tells `a` from `A`: then they are two choices.
    let keys: Vec<String> = found.iter().map(|(_, k)| if found.iter().any(|(_, o)| o != k && o.eq_ignore_ascii_case(k)) { k.clone() } else { k.to_lowercase() }).collect();
    let consents = found.iter().any(|(w, _)| matches!(w.as_str(), "yes" | "allow" | "approve" | "always"));
    if keys.len() >= 2 && consents {
        let question = lines.iter().rev().take(6).find(|l| l.trim_end().ends_with('?')).map(|l| clean(l));
        return Some(Prompt { keys: dedup(keys), question, line: false, lasting: Vec::new(), selected: None });
    }
    None
}

fn clean(line: &str) -> String {
    line.trim().trim_start_matches(['│', '|', '▌', '>', ' ']).trim().chars().take(160).collect()
}

fn dedup(keys: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for k in keys {
        if !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(screen: &str) -> Option<Vec<String>> {
        detect(screen).map(|p| p.keys)
    }

    #[test]
    fn choices_that_outlast_the_prompt_are_named() {
        let lasting = |s: &str| detect(s).unwrap().lasting;
        assert_eq!(lasting("Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again for npm test commands in D:\\proj\n   3. No, and tell Claude what to do differently (esc)\n"), ["2"]);
        assert_eq!(lasting("Do you want to make this edit?\n ❯ 1. Yes\n   2. Yes, allow all edits during this session (shift+tab)\n   3. No\n"), ["2"]);
        assert_eq!(lasting("Allow execution of: 'git status'?\n● 1. Yes, allow once\n  2. Yes, allow always\n  3. No, suggest changes (esc)\n"), ["2"]);
        assert_eq!(lasting("▌Allow command?\n▌ $ cargo build\n▌ Yes (y)   Always (a)   No, provide feedback (n)\n"), ["a"]);
        assert_eq!(lasting("Add file to the chat? (Y)es/(N)o/(D)on't ask again [Yes]: \n"), ["d"]);
        // Trust is kept: its yes lasts, whatever it is called.
        assert_eq!(lasting("Do you trust the files in this folder?\n ❯ 1. Yes, proceed\n   2. No, exit\n"), ["1"]);
        // Nothing that lasts: nothing named.
        assert!(lasting("Continue? (y/n)\n").is_empty());
    }

    /// Menus built from the labels of the installed agents, or traced from their shape.
    #[test]
    fn a_lasting_choice_has_no_way_to_be_answered_from_a_phone() {
        let lasting = |s: &str| detect(s).unwrap().lasting;
        assert_eq!(lasting("Allow network access to example.com?\n ❯ 1. Yes, just this once (y)\n   2. No, and block this host in the future\n   3. No, and tell Codex what to do differently (esc)\n"), ["2"]);
        assert_eq!(lasting("Resume this conversation?\n ❯ 1. Resume\n   2. Don't ask me again\n"), ["2"]);
        assert_eq!(lasting("Enable auto mode?\n ❯ 1. OK\n   2. Don't show again\n"), ["2"]);
        assert_eq!(lasting("Always allow this tool?\n ❯ 1. Yes\n   2. No\n"), ["1"]);
        assert_eq!(lasting("Allow this tool?\n ❯ 1. Enable\n   2. Enable for this chat\n   3. Decline\n"), ["2"]);
        assert_eq!(lasting("Allow this tool?\n ❯ 1. Allow (this chat)\n   2. Yes (until you restart)\n   3. No\n"), ["1", "2"]);
        assert_eq!(lasting("Use this tool?\n ❯ 1. Grant\n   2. Use for this chat\n   3. Turn on for good\n   4. No\n"), ["2", "3"]);
        assert_eq!(lasting("Remember this for all projects.\nContinue? (y/n)\n"), ["y"]);
        assert_eq!(lasting("Do you trust this folder?\nContinue? (y/n)\n"), ["y"]);
        assert_eq!(lasting("Set up cloud access\n ❯ 1. Yes, run hooks for cloud sessions\n   2. Yes, use my enabled plugins in cloud sessions\n   3. No\n"), ["1", "2"]);
        assert_eq!(lasting("Do you want to proceed?\n ❯ 1. Yes (y)\n   2. Yes, and don't ask again (a)\n   3. No (n)\n"), ["2", "a"], "by its number and by its letter");
    }

    /// An ordinary question stays answerable, whatever common word it or the output above it contains.
    #[test]
    fn ordinary_menus_keep_their_buttons_on_common_words() {
        let lasting = |s: &str| detect(s).unwrap().lasting;
        for menu in [
            "Should I run all the tests?\n ❯ 1. Yes\n   2. No\n",
            "Fixed all three warnings.\nShould I commit?\n ❯ 1. Yes\n   2. No\n",
            "Ran the session cleanup.\nShip it?\n ❯ 1. Yes, ship it\n   2. No\n",
            "Deleted all logs.\nContinue? (y/n)\n",
            "Which tests?\n ❯ 1. All\n   2. Changed only\n",
            "Which style?\n ❯ 1. Block\n   2. Inline\n",
            "Session cookies or JWT?\n ❯ 1. Session cookies\n   2. JWT\n",
            "How should I proceed?\n ❯ 1. Run the tests first\n      They take a minute\n   2. Use the old API\n      It still works\n   3. Type something.\n",
            "Should I set up CI for this project?\n ❯ 1. Yes\n   2. No\n",
            "Updated the README for this project.\nShould I commit?\n ❯ 1. Yes\n   2. No\n",
            "Add a config for this repo? (y/n)\n",
            "Which test runner?\n ❯ 1. Vitest\n      Fits the setup for this project\n   2. Jest\n      Already in the repo\n",
            "Apply to the other files?\n ❯ 1. Yes, all of them\n   2. Just this one\n   3. No\n",
            "Which tests should I run?\n ❯ 1. Run all tests\n      Takes five minutes\n   2. Run the changed ones\n      Faster\n",
        ] {
            assert!(lasting(menu).is_empty(), "{menu}");
        }
        // The same choices without descriptions, or with a tool's line above, are a request.
        assert_eq!(lasting("How should I proceed?\n ❯ 1. Run the tests first\n   2. Use the old API\n   3. No\n"), ["1", "2"]);
        assert_eq!(lasting("● Bash(npm test)\nHow should I proceed?\n ❯ 1. Run the tests first\n      They take a minute\n   2. No\n      Stop\n"), ["1"]);
        // A strong word right over a question still makes its yes last.
        assert_eq!(lasting("Remember this for all projects.\nContinue?\n ❯ 1. Yes\n   2. No\n"), ["1"]);
    }

    #[test]
    fn what_is_cheap_to_tell_is_told() {
        let lasting = |s: &str| detect(s).unwrap().lasting;
        // A question of trust, however long the explanation under it.
        assert_eq!(lasting("Do you trust this folder?\nIt may contain scripts.\nKelpie cannot check them.\nThis is kept.\n ❯ 1. Yes, proceed\n   2. No, exit\n"), ["1"]);
        // The yes of a lasting question, by any word of consent.
        assert_eq!(lasting("Always allow this tool?\n ❯ 1. OK\n   2. Cancel\n"), ["1"]);
        assert_eq!(lasting("Do you trust this folder?\n ❯ 1. OK\n   2. Cancel\n"), ["1"]);
        // A line starting in lowercase is a wrap, not a description.
        assert_eq!(lasting("Allow this?\n ❯ 1. Yes\n   2. Allow\n      for this chat\n   3. No\n"), ["2"]);
        assert_eq!(lasting("May I run it?\n ❯ 1. Yes\n   2. Yes, keep running these\n      commands here\n   3. No\n"), ["2"]);
        // A refusal is a refusal up to its comma or dash.
        for label in ["Don't ask for this tool", "Skip permission checks", "No more prompts", "Don't prompt"] {
            assert_eq!(lasting(&format!("Proceed?\n ❯ 1. Yes\n   2. {label}\n")), ["2"], "{label}");
        }
        assert_eq!(lasting("Which one?\n ❯ 1. Don't ask for this tool\n   2. Fine\n"), ["1"], "strong on any question");
    }

    /// The accepted limit of recognition: a term in a description with no word that lasts is not told from a description.
    #[test]
    fn a_description_with_no_lasting_word_is_read_as_a_description() {
        assert!(detect("Allow this?\n ❯ 1. Yes\n      Applies to every chat\n   2. No\n").unwrap().lasting.is_empty());
    }

    /// The accepted limit of recognition: a scope named only in the question leaves its plain yes answerable. On the choice it counts.
    #[test]
    fn a_scope_named_only_in_the_question_leaves_a_plain_yes_answerable() {
        assert!(detect("Allow this for this project?\n ❯ 1. Yes\n   2. No\n").unwrap().lasting.is_empty());
        assert_eq!(detect("Allow this for this project?\n ❯ 1. Yes\n   2. Yes, for this project\n   3. No\n").unwrap().lasting, ["2"]);
        assert_eq!(detect("Which one?\n ❯ 1. Yes, for this project\n   2. Fine\n").unwrap().lasting, ["1"]);
    }

    /// What is hidden on a phone among the labels of the installed agents, on an ordinary question and on one that names permission. Hold
    /// any change of the rules against this: a row that moves is a real menu that changes.
    #[test]
    fn real_labels() {
        // (label, hidden on `Which one?`, hidden on `Allow this?`)
        let rows: &[(&str, bool, bool)] = &[
            // Claude Code 2.1.314.
            ("Accept terms · Help improve our AI models: OFF", true, true),
            ("Accept terms · Help improve our AI models: OFF (for emails with your domain)", true, true),
            ("Accept terms · Help improve our AI models: ON", true, true),
            ("Allow", false, false),
            ("Allow carve-outs", true, true),
            ("Allow possible downgrade to stable version", true, true),
            ("Always copy full response", true, true),
            ("Continue", false, false),
            ("Continue automatically at usage limit", true, true),
            ("Continue now at lower priority", true, true),
            ("Continue this session in the cloud", true, true),
            ("Continue with browser tools", true, true),
            ("Continue with usage credits", true, true),
            ("Continue without browser tools", true, true),
            ("Continue without these settings", true, true),
            ("Continue without using this MCP server", true, true),
            ("Deny — drop it and tell the sender it was declined", false, false),
            ("Disable", false, true),
            ("Disable all LSP recommendations", false, true),
            ("Don't ask again", true, true),
            ("Don't ask me again", true, true),
            ("Don't show again", true, true),
            ("Enable", false, false),
            ("Enable Remote Control", false, true),
            ("Enable Remote Control for all sessions", true, true),
            ("Enable with latest channel", false, true),
            ("Enable with stable channel", false, true),
            ("Exit and fix manually", false, true),
            ("Exit and stop tasks", false, true),
            ("Exit without making changes", false, true),
            ("Keep waiting", false, true),
            ("Keep worktree", false, true),
            ("Keep worktree and tmux session", false, true),
            ("Keep worktree, end tmux session", false, true),
            ("Never mind", false, true),
            ("No", false, false),
            ("No Sandbox", false, true),
            ("No — ask me first, as in Manual mode", false, false),
            ("No advisor", false, true),
            ("No, and don't show plugin installation hints again", true, true),
            ("No, don't run hooks for cloud sessions", true, true),
            ("No, keep it", false, false),
            ("No, keep planning", false, false),
            ("No, keep them on this machine", false, false),
            ("No, keep them on this machine only", false, false),
            ("No, keep them to this machine", false, false),
            ("No, not now", false, false),
            ("No, refine with Ultraplan in a cloud session", true, true),
            ("Run and only show findings here", true, true),
            ("Run and post the findings to the PR as me", true, true),
            ("Save to file", false, true),
            ("Set up GitHub Actions workflows", false, true),
            ("Skip", false, false),
            ("Skip — use Claude Code defaults (auto-updates)", false, false),
            ("Skip for now (you can run /install-github-app again later)", false, true),
            ("Skip the /copy picker", false, true),
            ("Skip workflow update (configure secrets only)", false, true),
            ("Stop ultraplan", false, true),
            ("Stop ultrareview", false, true),
            ("Turn off suggestions", false, true),
            ("Turn on", false, false),
            ("Turn them on", false, true),
            ("Use a {kind} ({left} left, until {date})", false, true),
            ("Use auto mode during plan", true, true),
            ("Use credentials already in my environment", false, true),
            ("Use custom API key:", false, true),
            ("Use this MCP server", false, true),
            ("Use this and all future MCP servers in this project", true, true),
            ("Use this machine's settings in cloud sessions", true, true),
            ("Use tmux instead", false, true),
            ("Yes", false, false),
            ("Yes, and don’t ask again for", true, true),
            ("Yes, and remember this directory", true, true),
            ("Yes, delete it", false, true),
            ("Yes, for this session", true, true),
            ("Yes, offer this machine's MCP servers", false, true),
            ("Yes, register this computer again", false, true),
            ("Yes, run /web-setup", false, true),
            ("Yes, run hooks for cloud sessions", true, true),
            ("Yes, run it", false, false),
            ("Yes, send my settings", false, true),
            ("Yes, set auto mode as my default permission mode", true, true),
            ("Yes, use my enabled plugins in cloud sessions", true, true),
            // Also Claude Code, not in the scan's list of first words.
            ("Yes, continue", false, false),
            ("Yes, but ask again next time", false, false),
            // Codex.
            ("Yes, just this once (y)", false, false),
            ("No, continue without running it", false, false),
            ("No, and tell Codex what to do differently (esc)", false, false),
            ("No, and block this host in the future", true, true),
            ("Yes, and don't ask again for this command (a)", true, true),
        ];
        let hidden = |question: &str, label: &str| detect(&format!("{question}\n ❯ 1. {label}\n   2. Something else\n")).unwrap().lasting.iter().any(|k| k == "1");
        let wrong: Vec<String> = rows
            .iter()
            .filter(|(label, plain, permission)| hidden("Which one?", label) != *plain || hidden("Allow this?", label) != *permission)
            .map(|(label, plain, permission)| format!("{label}: expected {plain}/{permission}, got {}/{}", hidden("Which one?", label), hidden("Allow this?", label)))
            .collect();
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn a_phones_answer_to_a_prompt_with_a_lasting_choice_is_one_of_its_plain_choices() {
        let p = detect("Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again\n   3. No\n").unwrap();
        assert_eq!(p.selected.as_deref(), Some("1"));
        for ok in [(None, Some("1")), (None, Some("3")), (Some("1"), None), (Some("esc"), None), (Some("enter"), None), (None, Some(""))] {
            assert!(p.plain_answer(ok.0, ok.1), "{ok:?}");
        }
        for no in [(None, Some("2")), (Some("2"), None), (None, Some("2x")), (None, Some("2\u{200B}")), (None, Some("yes")), (None, Some("esc")), (None, Some(" 1"))] {
            assert!(!p.plain_answer(no.0, no.1), "{no:?}");
        }
        // The marker on the lasting one: Enter would choose it.
        let on_it = detect("Do you want to proceed?\n   1. Yes\n ❯ 2. Yes, and don't ask again\n   3. No\n").unwrap();
        assert_eq!(on_it.selected.as_deref(), Some("2"));
        assert!(!on_it.plain_answer(Some("enter"), None) && !on_it.plain_answer(None, Some("")));
        // No marker to go by: nor is Enter known to be safe.
        let y = detect("Always allow this command? (y/n)\n").unwrap();
        assert!(!y.plain_answer(Some("enter"), None) && !y.plain_answer(None, Some("yes")) && !y.plain_answer(None, Some("y")));
        assert!(y.plain_answer(None, Some("n")));
        // With nothing that lasts, anything goes as before.
        let plain = detect("Continue? (y/n)\n").unwrap();
        assert!(plain.plain_answer(None, Some("yes")) && plain.plain_answer(Some("enter"), None));
    }

    #[test]
    fn a_no_that_comes_back_next_time_stays_answerable() {
        let lasting = |s: &str| detect(s).unwrap().lasting;
        assert!(lasting("Allow this read?\n ❯ 1. Yes\n   2. No, and ask again next time\n").is_empty());
        assert!(lasting("Would you like to run the following command?\n ❯ 1. Yes, just this once (y)\n   2. No, continue without running it\n   3. No, and tell Codex what to do differently (esc)\n").is_empty());
        assert!(lasting("Run it?\n ❯ 1. Yes\n   2. Yes, run it\n   3. Yes, continue\n   4. No\n").is_empty());
    }

    #[test]
    fn an_action_alone_does_not_make_a_question_of_permission() {
        let lasting = |s: &str| detect(s).unwrap().lasting;
        assert!(lasting("Which file should I edit?\n ❯ 1. a.rs\n   2. b.rs\n").is_empty());
        let form = "Should I create a PR?\n ❯ 1. Yes, create it\n      Opens a PR against main\n   2. Yes\n      With a draft title\n   3. No\n      Leave the branch\n   4. Type something.\n";
        assert!(lasting(form).is_empty(), "consent-led choices with descriptions");
        // A word that lasts still hides.
        let lasts = "Should I create a PR?\n ❯ 1. Yes\n      Just this one\n   2. Yes, and always do\n      For every branch\n   3. No\n      Leave it\n";
        assert_eq!(lasting(lasts), ["2"]);
        // Asked plainly, with nothing to say it is not a request.
        assert_eq!(lasting("May I run it?\n ❯ 1. Yes\n   2. Yes, ship it\n   3. No\n"), ["2"]);
    }

    #[test]
    fn a_real_one_time_yes_with_a_condition_or_a_description_is_answerable() {
        let lasting = |s: &str| detect(s).unwrap().lasting;
        assert!(lasting("Allow this read outside the working directories?\n ❯ 1. Yes, but ask again next time\n   2. No\n").is_empty());
        assert!(lasting("Allow this read?\n ❯ 1. Yes\n      Reads one file\n   2. No\n      Stops here\n").is_empty(), "the first line is the answer");
        assert_eq!(lasting("Allow this read?\n ❯ 1. Yes\n      Reads one file\n   2. Yes, read everything from now on\n      No more asking\n   3. No\n"), ["2"]);
    }

    #[test]
    fn a_question_that_is_not_about_permission_keeps_its_buttons() {
        let lasting = |s: &str| detect(s).unwrap().lasting;
        assert!(lasting("Ship it?\n ❯ 1. Yes, ship it\n   2. No\n").is_empty());
        // The agent's question form: a description line under each option, and a way to say something else.
        let form = "Should I apply the fix?\n ❯ 1. Yes\n      Apply the fix\n   2. No\n      Leave it\n   3. Type something.\n";
        assert_eq!(detect(form).unwrap().keys, ["1", "2", "3"]);
        assert!(lasting(form).is_empty());
        // Such a menu with words that last still hides them.
        assert_eq!(lasting("Ship it?\n ❯ 1. Yes, ship it\n   2. Yes, and ship everything from now on\n   3. No\n"), ["2"]);
    }

    #[test]
    fn a_tool_line_or_a_question_of_permission_keeps_unknown_labels_refused() {
        let lasting = |s: &str| detect(s).unwrap().lasting;
        // The same labels as the question above, once the screen shows what is at stake.
        for above in ["Do you want to proceed?", "Allow this?", "May I run it?", "Bash(npm test)\nReady?", "● Edit(src/a.rs)\nShall I?", "$ cargo build\nGo?"] {
            assert_eq!(lasting(&format!("{above}\n ❯ 1. Yes\n   2. Yes, ship it\n   3. No\n")), ["2"], "{above}");
        }
        // Or the choices speak of it themselves.
        assert_eq!(lasting("Apply this change?\n ❯ 1. Yes, allow once\n   2. Modify with an editor\n   3. No\n"), ["2"]);
        assert_eq!(lasting("Next?\n ❯ 1. Proceed\n   2. Keep going without asking\n"), ["2"]);
        assert_eq!(lasting("Next?\n ❯ 1. Proceed\n   2. Something new\n"), ["2"], "a word of consent other than yes");
    }

    #[test]
    fn the_question_of_a_yes_or_no_can_be_the_lasting_part() {
        let lasting = |s: &str| detect(s).unwrap().lasting;
        assert_eq!(lasting("Always allow this command? (y/n)\n"), ["y"]);
        assert_eq!(lasting("Always allow this command?\n(y/n)\n"), ["y"]);
        assert_eq!(lasting("Allow from now on? [Y/n] "), ["y"]);
        assert!(lasting("Allow this command? (y/n)\n").is_empty());
    }

    #[test]
    fn keys_are_told_apart_by_case_only_where_the_prompt_does() {
        let p = detect("Permission required: edit src/main.rs\n  Allow (a)   Allow always (A)   Deny (d)\n").unwrap();
        assert_eq!(p.keys, ["a", "A", "d"]);
        assert_eq!(p.lasting, ["A"], "the once key stays answerable");
        // Where it does not, a capital is the same key.
        let p = detect("Allow?\n  Yes (Y)   Always (A)   No (N)\n").unwrap();
        assert_eq!(p.keys, ["y", "a", "n"]);
        assert_eq!(p.lasting, ["a"]);
    }

    #[test]
    fn the_labels_of_lasting_modes_are_known() {
        let lasting = |s: &str| detect(s).unwrap().lasting;
        for label in ["Yes, auto-accept edits", "Yes, and bypass permissions", "Yes, for this project", "Yes, from now on", "Allow all edits", "Yes, trust for this repository"] {
            assert_eq!(lasting(&format!("Do you want to proceed?\n ❯ 1. Yes\n   2. {label}\n   3. No\n")), ["2"], "{label}");
        }
    }

    #[test]
    fn an_item_wrapped_in_a_narrow_pane_is_read_whole() {
        let lasting = |s: &str| detect(s).unwrap().lasting;
        assert_eq!(lasting("Do you want to make this edit?\n ❯ 1. Yes\n   2. Yes, allow all edits during this\n      session (shift+tab)\n   3. No\n"), ["2"]);
        assert_eq!(lasting("Proceed?\n ❯ 1. Yes\n   2. Yes, keep it\n      for this project\n   3. No\n"), ["2"]);
        // The last item too; a hint below it is not its text.
        assert_eq!(lasting("Proceed?\n ❯ 1. No\n   2. Yes, and do not\n      ask again\n"), ["2"]);
        assert!(lasting("Proceed?\n ❯ 1. Yes\n   2. No\n Esc to cancel · session settings\n").is_empty());
    }

    #[test]
    fn on_a_question_of_permission_what_cannot_be_told_counts_as_lasting() {
        let lasting = |s: &str| detect(s).unwrap().lasting;
        assert_eq!(lasting("Allow this?\n ❯ 1. Yes\n   2. Yes, and manually approve edits\n   3. No, keep planning\n"), ["2"]);
        assert_eq!(lasting("Allow this?\n ❯ 1. Allow\n   2. Something new an agent came up with\n   3. Deny\n"), ["2"]);
        // Plain once and plain no, in the wordings agents use.
        assert!(lasting("Allow this?\n ❯ 1. Yes, allow once\n   2. No, suggest changes (esc)\n").is_empty());
        assert!(lasting("Allow this?\n ❯ 1. Allow\n   2. Reject\n").is_empty());
        assert!(lasting("Allow this?\n ❯ 1. Yes, proceed\n   2. No, and tell Claude what to do differently (esc)\n").is_empty());
        // A choice that is not about permission is the phone's to make.
        assert!(lasting("Which approach?\n ❯ 1. Rewrite it\n   2. Patch it\n").is_empty());
        // No numbers: every key beyond yes and no.
        assert_eq!(lasting("Confirm\nSure?\n[Y] Yes  [A] Yes to All  [N] No  [L] No to All  [S] Suspend  [?] Help (default is \"Y\"): "), ["a", "l", "s", "?"]);
        assert_eq!(lasting("Add file? (Y)es/(N)o/(A)ll/(S)kip all/(D)on't ask again [Yes]: "), ["a", "s", "d"]);
        assert_eq!(lasting("(1/2) Stage this hunk [y,n,q,a,d,e,?]? "), ["q", "a", "d", "e", "?"]);
    }

    #[test]
    fn claude_permission_menu() {
        let s = "● Bash(npm test)\n\n Bash command\n   npm test\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again for npm test commands in D:\\proj\n   3. No, and tell Claude what to do differently (esc)\n";
        let p = detect(s).unwrap();
        assert_eq!(p.keys, ["1", "2", "3"]);
        assert_eq!(p.question.as_deref(), Some("Do you want to proceed?"));
    }

    #[test]
    fn claude_menu_in_a_box_with_a_hint_below() {
        let s = "╭──────────────╮\n│ Do you trust the files in this folder?\n│ ❯ 1. Yes, proceed\n│   2. No, exit\n╰──────────────╯\n   Enter to confirm · Esc to exit\n";
        assert_eq!(keys(s).unwrap(), ["1", "2"]);
    }

    #[test]
    fn gemini_menu() {
        let s = "Allow execution of: 'git status'?\n● 1. Yes, allow once\n  2. Yes, allow always\n  3. No, suggest changes (esc)\n";
        assert_eq!(keys(s).unwrap(), ["1", "2", "3"]);
    }

    #[test]
    fn codex_approval() {
        let s = "▌Allow command?\n▌ $ cargo build\n▌ Yes (y)   Always (a)   No, provide feedback (n)\n";
        assert_eq!(keys(s).unwrap(), ["y", "a", "n"]);
    }

    #[test]
    fn aider_question() {
        let s = "Add src/app.ts to the chat? (Y)es/(N)o/(D)on't ask again [Yes]:";
        assert_eq!(keys(s).unwrap(), ["y", "n", "d"]);
    }

    #[test]
    fn line_prompts_need_enter_and_menus_do_not() {
        assert!(detect("Continue (y/n): ").unwrap().line);
        assert!(detect("Add a.ts to the chat? (Y)es/(N)o [Yes]:").unwrap().line);
        assert!(!detect("Proceed?\n❯ 1. Yes\n  2. No\n").unwrap().line);
        assert!(!detect("▌ Yes (y)   No, provide feedback (n)\n").unwrap().line);
    }

    #[test]
    fn an_idle_shell_after_printing_a_question_and_a_list_is_not_waiting() {
        let s = "PS D:\\proj> type q.txt\nWhich one do you want?\n1. Back up\n2. Delete\nPS D:\\proj> ";
        assert_eq!(detect(s), None);
        assert_eq!(detect("$ cat q.txt\nWhich?\n1. a\n2. b\nuser@box:~/proj$ "), None);
        assert_eq!(detect("C:\\proj> type q.txt\nWhich?\n1. a\n2. b\nC:\\proj>"), None);
    }

    #[test]
    fn an_agents_numbered_question_with_its_input_box_below_is_not_a_menu() {
        let s = "Which approach do you prefer?\n1. Rewrite it\n2. Patch it\n\n▌ I think the second\n";
        assert_eq!(detect(s), None);
        assert_eq!(detect("Which approach?\n1. Rewrite\n2. Patch\n> "), None);
    }

    #[test]
    fn powershell_confirmation_git_add_p_and_opencode() {
        let ps = "Confirm\nAre you sure you want to perform this action?\n[Y] Yes  [A] Yes to All  [N] No  [L] No to All  [S] Suspend  [?] Help (default is \"Y\"): ";
        let p = detect(ps).unwrap();
        assert_eq!(p.keys, ["y", "a", "n", "l", "s", "?"]);
        assert!(p.line);
        let git = "@@ -1,3 +1,4 @@\n+new line\n(1/2) Stage this hunk [y,n,q,a,d,e,?]? ";
        assert_eq!(keys(git).unwrap(), ["y", "n", "q", "a", "d", "e", "?"]);
        let oc = "Permission required: edit src/main.rs\n  Allow (a)   Allow always (A)   Deny (d)\n";
        assert_eq!(keys(oc).unwrap(), ["a", "A", "d"], "two different choices");
    }

    #[test]
    fn the_screen_identifies_a_prompt_up_to_its_last_line() {
        let before = "Bash(rm -rf build)\nDo you want to proceed?\n❯ 1. Yes\n  2. No\n";
        let after = "Bash(rm -rf build)\nDo you want to proceed?\n❯ 1. Yes\n  2. No\nANSWERED\n";
        let other = "Bash(npm test)\nDo you want to proceed?\n❯ 1. Yes\n  2. No\n";
        let (_, a) = detect_with_screen(before).unwrap();
        let (_, b) = detect_with_screen(after).unwrap();
        let (_, c) = detect_with_screen(other).unwrap();
        assert_eq!(a, b, "output below the answered menu is not part of it");
        assert_ne!(a, c, "the same menu about another command is another prompt");
    }

    #[test]
    fn plain_yes_no() {
        assert_eq!(keys("PS D:\\proj> .\\deploy.ps1\nContinue (y/n): ").unwrap(), ["y", "n"]);
        assert_eq!(keys("Overwrite existing file? [Y/n] ").unwrap(), ["y", "n"]);
        assert_eq!(keys("Proceed? (yes/no)").unwrap(), ["y", "n"]);
    }

    #[test]
    fn opencode_numbered_with_question() {
        let s = "Permission required: edit src/main.rs?\n1. Allow\n2. Allow always\n3. Reject\n";
        assert_eq!(keys(s).unwrap(), ["1", "2", "3"]);
    }

    #[test]
    fn shell_prompt_after_text_mentioning_yes_no() {
        let s = "PS D:\\proj> type notes.md\nAsk the user (y/n) before deleting.\n1. Back up\n2. Delete\nPS D:\\proj> ";
        assert_eq!(detect(s), None);
    }

    #[test]
    fn numbered_prose_and_lists_are_not_menus() {
        let s = "Steps:\n1. Install the CLI\n2. Run it\n3. Profit\n\nAll done. Anything else?\n> ";
        assert_eq!(detect(s), None);
        assert_eq!(detect("The plan:\n1. Parse\n2. Check\nI'll start with step 1 now."), None);
    }

    #[test]
    fn diff_and_logs_are_not_prompts() {
        let s = "diff --git a/x b/x\n-  if ok (y/n)\n+  if ok (y/n)?\n@@ -1,2 +1,2 @@\n context\n";
        assert_eq!(detect(s), None);
        assert_eq!(detect("   Compiling kelpied v0.1.2\n    Finished `dev` profile in 4.2s\n"), None);
    }

    #[test]
    fn menu_must_start_at_one_and_be_consecutive() {
        assert_eq!(detect("Which?\n❯ 2. b\n  3. c\n"), None);
        assert_eq!(detect("Which?\n❯ 1. a\n  3. c\n"), None);
        assert_eq!(keys("Which?\n❯ 1. a\n  2. b\n").unwrap(), ["1", "2"]);
    }

    #[test]
    fn a_menu_far_above_the_cursor_is_old() {
        let s = "Do you want to proceed?\n❯ 1. Yes\n  2. No\nok\nrunning tests\npassed 10\npassed 11\nsummary\n";
        assert_eq!(detect(s), None);
    }

    #[test]
    fn empty_screen() {
        assert_eq!(detect(""), None);
        assert_eq!(detect("\n\n  \n"), None);
    }

    #[test]
    fn detection_is_cheap() {
        let screen = "x".repeat(120) + "\n";
        let screen = screen.repeat(40) + "Do you want to proceed?\n❯ 1. Yes\n  2. No\n";
        detect(&screen);
        let t = std::time::Instant::now();
        for _ in 0..200 {
            assert!(detect(&screen).is_some());
        }
        // Generous for debug builds and loaded CI machines; release is far below 0.2 ms.
        assert!(t.elapsed().as_secs_f64() / 200.0 < 0.005, "{:?} per check", t.elapsed() / 200);
    }
}
