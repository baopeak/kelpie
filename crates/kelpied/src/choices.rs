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
}

/// How far up a menu may start, in non-empty lines from the bottom.
const MENU_LINES: usize = 12;
/// A yes/no prompt must sit on one of the last lines.
const YES_NO_LINES: usize = 3;

/// `❯ 1. Yes`, `● 2) No`, `  3. No, and tell Claude what to do differently (esc)`.
static MENU_ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:[│|]\s*)?(?:[❯>›»▶●○◉◯*]\s*)?(\d)[.)]\s+\S").unwrap());
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
    let (prompt, end) = menu(&lines).or_else(|| yes_no(&lines).map(|p| (p, lines.len() - 1)))?;
    Some((prompt, lines[..=end].join("\n")))
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
    Some((Prompt { keys: items.iter().map(|(_, n)| n.to_string()).collect(), question, line: false }, start + last_item))
}

fn yes_no(lines: &[&str]) -> Option<Prompt> {
    let tail = &lines[lines.len().saturating_sub(YES_NO_LINES)..];
    let last = *tail.last()?;
    if YES_NO.is_match(last) {
        return Some(Prompt { keys: vec!["y".into(), "n".into()], question: Some(clean(last)), line: true });
    }
    let bracket: Vec<String> = BRACKET_KEYS.captures_iter(last).map(|c| c[1].to_lowercase()).collect();
    if bracket.len() >= 2 && bracket.contains(&"y".to_string()) && bracket.contains(&"n".to_string()) {
        return Some(Prompt { keys: dedup(bracket), question: Some(clean(last)), line: true });
    }
    if let Some(c) = KEY_LIST.captures(last) {
        let keys: Vec<String> = c[1].split(',').map(str::to_string).collect();
        if keys.contains(&"y".to_string()) && keys.contains(&"n".to_string()) {
            return Some(Prompt { keys, question: Some(clean(last)), line: true });
        }
    }
    let initials: Vec<String> = PAREN_INITIALS.captures_iter(last).map(|c| c[1].to_lowercase()).collect();
    if initials.len() >= 2 && initials.contains(&"y".to_string()) && initials.contains(&"n".to_string()) {
        return Some(Prompt { keys: dedup(initials), question: Some(clean(last)), line: true });
    }
    let found: Vec<(String, String)> = tail.iter().flat_map(|l| TRAILING_KEY.captures_iter(l).map(|c| (c[1].to_lowercase(), c[2].to_lowercase())).collect::<Vec<_>>()).collect();
    let keys: Vec<String> = found.iter().map(|(_, k)| k.clone()).collect();
    let consents = found.iter().any(|(w, _)| matches!(w.as_str(), "yes" | "allow" | "approve" | "always"));
    if keys.len() >= 2 && consents {
        let question = lines.iter().rev().take(6).find(|l| l.trim_end().ends_with('?')).map(|l| clean(l));
        return Some(Prompt { keys: dedup(keys), question, line: false });
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
        assert_eq!(keys(oc).unwrap(), ["a", "d"]);
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
