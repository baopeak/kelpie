//! Names a target can hold: Windows' forbidden characters and device names, "keep both" naming, names that
//! differ only in case, and the hidden partial a transfer writes before the rename into place.

/// Characters Windows refuses in a name, besides controls.
const WINDOWS_FORBIDDEN: &[char] = &['<', '>', ':', '"', '\\', '|', '?', '*', '/'];

/// `name` as Windows can store it, and whether it had to change (the queue reports "saved as …").
/// Forbidden characters and controls become `_`; a trailing dot or space becomes `_`; a device name
/// (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, with any extension) gets `_` after its stem.
pub fn windows_safe(name: &str) -> (String, bool) {
    let mut out: String = name.chars().map(|c| if c.is_control() || WINDOWS_FORBIDDEN.contains(&c) { '_' } else { c }).collect();
    let kept = out.trim_end_matches(['.', ' ']).len();
    if kept < out.len() {
        let tail = out.len() - kept;
        out.truncate(kept);
        out.extend(std::iter::repeat_n('_', tail));
    }
    if out.is_empty() {
        out.push('_');
    }
    let stem_len = out.find('.').unwrap_or(out.len());
    if is_device_name(&out[..stem_len]) {
        out.insert(stem_len, '_');
    }
    let changed = out != name;
    (out, changed)
}

fn is_device_name(stem: &str) -> bool {
    let s = stem.trim_end_matches(' ').to_ascii_uppercase();
    matches!(s.as_str(), "CON" | "PRN" | "AUX" | "NUL") || (s.len() == 4 && (s.starts_with("COM") || s.starts_with("LPT")) && matches!(s.as_bytes()[3], b'1'..=b'9'))
}

/// `name` with ` (n)` before its extension: `report (2).pdf`, `archive.tar (2).gz`, `.bashrc (2)`, `notes (2)`.
pub fn numbered(name: &str, n: u32) -> String {
    with_suffix(name, &format!(" ({n})"))
}

/// `name` with `suffix` before its extension (a leading dot is part of the name, not an extension).
pub fn with_suffix(name: &str, suffix: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{}{suffix}{}", &name[..i], &name[i..]),
        _ => format!("{name}{suffix}"),
    }
}

/// The first `name (n)` (n ≥ 2) that `taken` does not hold. On a case-insensitive target `taken` compares
/// with [`same_name`], so a name that differs only in case counts as taken.
pub fn keep_both(name: &str, taken: impl Fn(&str) -> bool) -> String {
    (2..).map(|n| numbered(name, n)).find(|c| !taken(c)).expect("an unused name")
}

/// Whether two names are the same file on a target that is (or is not) case-insensitive.
pub fn same_name(a: &str, b: &str, case_insensitive: bool) -> bool {
    if case_insensitive {
        a.to_lowercase() == b.to_lowercase()
    } else {
        a == b
    }
}

/// Whether this computer's file systems usually ignore case (Windows, and macOS by default).
pub const LOCAL_CASE_INSENSITIVE: bool = cfg!(any(windows, target_os = "macos"));

/// The hidden file a transfer writes beside its target, renamed into place when complete.
pub fn partial_name(name: &str) -> String {
    format!(".{name}.kelpie-part")
}

pub fn is_partial(name: &str) -> bool {
    name.starts_with('.') && name.ends_with(".kelpie-part") && name.len() > ".kelpie-part".len() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_names() {
        assert_eq!(windows_safe("report.pdf"), ("report.pdf".into(), false));
        assert_eq!(windows_safe("a:b"), ("a_b".into(), true));
        assert_eq!(windows_safe("what?<>|*\".txt"), ("what______.txt".into(), true));
        assert_eq!(windows_safe("x."), ("x_".into(), true));
        assert_eq!(windows_safe("x. "), ("x__".into(), true));
        assert_eq!(windows_safe("tab\there"), ("tab_here".into(), true));
        assert_eq!(windows_safe("CON.txt"), ("CON_.txt".into(), true));
        assert_eq!(windows_safe("con"), ("con_".into(), true));
        assert_eq!(windows_safe("lpt9.tar.gz"), ("lpt9_.tar.gz".into(), true));
        assert_eq!(windows_safe("COM0.txt"), ("COM0.txt".into(), false));
        assert_eq!(windows_safe("console.log"), ("console.log".into(), false));
        assert_eq!(windows_safe("..."), ("___".into(), true));
        assert_eq!(windows_safe("ünïcødé 日本語.md"), ("ünïcødé 日本語.md".into(), false));
    }

    #[test]
    fn numbered_names() {
        assert_eq!(numbered("report.pdf", 2), "report (2).pdf");
        assert_eq!(numbered("archive.tar.gz", 3), "archive.tar (3).gz");
        assert_eq!(numbered(".bashrc", 2), ".bashrc (2)");
        assert_eq!(numbered("notes", 2), "notes (2)");
        assert_eq!(with_suffix("main.rs", " (edited)"), "main (edited).rs");
    }

    #[test]
    fn keep_both_skips_taken_names() {
        let taken = ["a.txt", "a (2).txt", "A (3).TXT"];
        let exact = keep_both("a.txt", |c| taken.contains(&c));
        assert_eq!(exact, "a (3).txt");
        let folded = keep_both("a.txt", |c| taken.iter().any(|t| same_name(t, c, true)));
        assert_eq!(folded, "a (4).txt");
    }

    #[test]
    fn case_pairs() {
        assert!(same_name("Readme.md", "README.md", true));
        assert!(!same_name("Readme.md", "README.md", false));
        assert!(same_name("ß", "ß", false));
    }

    #[test]
    fn partials() {
        assert_eq!(partial_name("big.iso"), ".big.iso.kelpie-part");
        assert!(is_partial(".big.iso.kelpie-part"));
        assert!(!is_partial(".kelpie-part"));
        assert!(!is_partial("big.iso"));
    }
}
