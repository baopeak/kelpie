use super::*;
use crate::agent_history::claude_project_dir;
use serde_json::json;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("session-export")
}

/// Fixture name, agent, conversation id and the title the Sessions panel shows for it.
const CASES: [(&str, Agent, &str, &str); 3] = [
    ("claude", Agent::Claude, "0c1d2e3f-0000-4000-8000-000000000001", "Fix the flaky build"),
    ("codex", Agent::Codex, "019a1b2c-3d4e-7f00-8000-000000000002", "Move the API key out of config.ts"),
    ("omp", Agent::Omp, "omp-fixture-0003", "Clean up the logger"),
];

/// Seeded in the fixtures (fake, but in the shapes `redact` knows).
const SECRETS: [&str; 6] = [
    "ghp_FAKEfakeFAKEfakeFAKEfakeFAKEfake1234",
    "sk-proj-FAKEfakeFAKEfake0123456789",
    "abcdEFGHijklMNOPqrst1234",
    "Sup3rS3cretPw",
    "wJalrXUtnFEMIbPxRfiCYFAKEKEY99",
    "hunter2hunter2",
];

fn export_fixture(name: &str, agent: Agent, id: &str, title: &str) -> Rendered {
    render(agent, &fixtures().join(format!("{name}.jsonl")), &Request { id, cwd: None, title: Some(title) }).unwrap()
}

fn render_vec(agent: Agent, lines: Vec<Result<String, u64>>, max_body: usize) -> Rendered {
    let mut it = lines.into_iter();
    render_lines(agent, || it.next(), &Request { id: "t1", cwd: Some("/w"), title: None }, max_body, None)
}

fn json_lines_of(values: &[Value]) -> Vec<Result<String, u64>> {
    values.iter().map(|v| Ok(v.to_string())).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kelpie-session-export-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn exports_equal_the_golden_files() {
    for (name, agent, id, title) in CASES {
        let want = std::fs::read_to_string(fixtures().join(format!("{name}.md"))).unwrap();
        assert!(!want.contains('\r'), "{name}.md must have LF line endings");
        let got = export_fixture(name, agent, id, title);
        assert_eq!(got.text, want, "{name}: the export differs from {name}.md");
        assert!(!got.text.contains('\r'), "{name}: CR in the export");
        assert!(!got.truncated);
    }
}

/// `cargo test -p kelpie --lib session_export::tests::bless -- --ignored`: rewrites the golden files
/// from the current output (review the diff before committing it).
#[test]
#[ignore]
fn bless() {
    for (name, agent, id, title) in CASES {
        std::fs::write(fixtures().join(format!("{name}.md")), export_fixture(name, agent, id, title).text).unwrap();
    }
}

#[test]
fn fixture_secrets_never_reach_the_export() {
    let mut seen = [false; SECRETS.len()];
    for (name, agent, id, title) in CASES {
        let raw = std::fs::read_to_string(fixtures().join(format!("{name}.jsonl"))).unwrap();
        let out = export_fixture(name, agent, id, title).text;
        for (i, s) in SECRETS.iter().enumerate() {
            seen[i] |= raw.contains(s);
            assert!(!out.contains(s), "{name}: {s} leaked");
        }
        assert!(out.contains(kelpie_proto::redact::MASK), "{name}: nothing was masked");
    }
    assert!(seen.iter().all(|s| *s), "every secret is seeded in some fixture");
}

#[test]
fn secrets_in_every_part_of_a_turn_are_masked() {
    let (gh, sk, pw, bearer) = (SECRETS[0], SECRETS[1], SECRETS[5], SECRETS[2]);
    let claude = json_lines_of(&[
        json!({"type": "user", "message": {"role": "user", "content": format!("user text {gh}")}}),
        json!({"type": "assistant", "message": {"role": "assistant", "content": [
            {"type": "text", "text": format!("assistant text {sk}")},
            {"type": "tool_use", "id": "t", "name": "Bash", "input": {"command": format!("DB_PASSWORD={pw} ./deploy"), "password": pw}},
        ]}}),
        json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t", "content": format!("tool output Authorization: Bearer {bearer}")}]}}),
    ]);
    let codex = json_lines_of(&[
        json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": format!("user text {gh}")}]}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": format!("assistant text {sk}")}]}}),
        json!({"type": "response_item", "payload": {"type": "function_call", "name": "shell", "call_id": "c", "arguments": json!({"command": ["bash", "-lc", format!("export API_KEY={pw}")]}).to_string()}}),
        json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "c", "output": format!("tool output Authorization: Bearer {bearer}")}}),
    ]);
    let omp = json_lines_of(&[
        json!({"type": "message", "message": {"role": "user", "content": format!("user text {gh}")}}),
        json!({"type": "message", "message": {"role": "assistant", "content": [
            {"type": "text", "text": format!("assistant text {sk}")},
            {"type": "toolCall", "id": "c", "name": "bash", "arguments": {"command": format!("curl https://u:{pw}@host/x")}},
        ]}}),
        json!({"type": "message", "message": {"role": "toolResult", "toolCallId": "c", "toolName": "bash", "content": [{"type": "text", "text": format!("tool output Authorization: Bearer {bearer}")}]}}),
    ]);
    for (agent, lines) in [(Agent::Claude, claude), (Agent::Codex, codex), (Agent::Omp, omp)] {
        let out = render_vec(agent, lines, MAX_BODY).text;
        for s in [gh, sk, pw, bearer] {
            assert!(!out.contains(s), "{agent:?}: {s} leaked:\n{out}");
        }
        // Every part is still there, only masked.
        for part in ["user text [redacted]", "assistant text [redacted]", "Tool call:", "tool output Authorization: Bearer [redacted]"] {
            assert!(out.contains(part), "{agent:?}: missing {part:?}:\n{out}");
        }
    }
}

#[test]
fn tool_text_is_cut_with_a_marker() {
    let many = (1..=100).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let out = cut(&many, 0);
    assert!(out.starts_with("line 1\n") && out.ends_with("line 40\n… 60 lines omitted"), "{out}");
    assert_eq!(cut("short\ntext", 0), "short\ntext");
    // The character allowance stops before the line cap: 200-character lines, 19 fit in 4000.
    let wide = (0..30).map(|_| "w".repeat(200)).collect::<Vec<_>>().join("\n");
    assert!(cut(&wide, 0).ends_with("\n… 11 lines omitted"));
    // One huge line keeps its start.
    let huge = "x".repeat(10_000);
    assert_eq!(cut(&huge, 0), format!("{}…", "x".repeat(CUT_CHARS)));
    assert_eq!(cut(&format!("{huge}\nnext"), 0), format!("{}…\n… 1 line omitted", "x".repeat(CUT_CHARS)));
    // Lines after the redacted window count too.
    assert!(cut("a\nb", 5).ends_with("b\n… 5 lines omitted"));
}

#[test]
fn a_fence_outlasts_the_backticks_inside() {
    let mut doc = Doc::default();
    doc.details("Result", "before ```` after", "text");
    assert!(doc.body.contains("`````text\nbefore ```` after\n`````\n"), "{}", doc.body);
    assert_eq!(code("a`b"), "``a`b``");
    assert_eq!(code("`x"), "`` `x ``");
}

#[test]
fn malformed_and_partial_lines_are_skipped() {
    let user = |t: &str| json!({"type": "user", "timestamp": "2026-01-01T00:00:00Z", "message": {"role": "user", "content": t}}).to_string();
    let lines = vec![
        Ok("not json".into()),
        Ok(String::new()),
        Ok("42".into()),
        Ok(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"cut"#.into()),
        Ok(r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#.into()),
        Ok(r#"{"type":"assistant","message":{"content":"plain string reply"}}"#.into()),
        Err(40 << 20),
        Ok(user("still here")),
    ];
    let r = render_vec(Agent::Claude, lines, MAX_BODY);
    assert_eq!(r.skipped, 3, "{}", r.text);
    assert!(r.text.contains("## User\n\nstill here\n"), "{}", r.text);
    assert!(r.text.contains("plain string reply"));
    assert!(r.text.ends_with("*3 transcript lines could not be read and are left out.*\n"), "{}", r.text);
    // Odd shapes in the other formats are passed over too.
    for agent in [Agent::Codex, Agent::Omp] {
        let odd = vec![Ok(r#"{"type":"response_item","payload":{"type":"message","content":"x"}}"#.into()), Ok(r#"{"type":"message","message":{"role":"toolResult","content":7}}"#.into()), Ok("[1,2]".into())];
        let r = render_vec(agent, odd, MAX_BODY);
        assert_eq!(r.skipped, 0, "{agent:?}");
    }
    // Nothing readable at all.
    assert!(render_vec(Agent::Omp, vec![], MAX_BODY).text.contains("*No messages.*"));
}

#[test]
fn the_reader_skips_long_lines_and_keeps_an_unterminated_last_one() {
    let dir = scratch("reader");
    let path = dir.join("t.jsonl");
    std::fs::write(&path, format!("short\n{}\nafter\r\nlast", "y".repeat(100))).unwrap();
    let mut lines = open_lines(&path, 0, u64::MAX).unwrap();
    assert_eq!(lines.next_capped(50), Some(Ok("short".into())));
    assert_eq!(lines.next_capped(50), Some(Err(101)));
    assert_eq!(lines.next_capped(50), Some(Ok("after".into())));
    assert_eq!(lines.next_capped(50), Some(Ok("last".into())));
    assert_eq!(lines.next_capped(50), None);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_export_stops_at_its_size_cap() {
    let line = |i: usize| json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": format!("reply {i} {}", "z".repeat(500))}]}});
    let r = render_vec(Agent::Claude, json_lines_of(&(0..20).map(line).collect::<Vec<_>>()), 2000);
    assert!(r.truncated);
    assert!(r.text.contains("reply 3 ") && !r.text.contains("reply 4 "), "{}", r.text);
    assert!(r.text.ends_with("*The export stops here: the conversation is longer than 2000 bytes of Markdown.*\n"));
    assert_eq!(size(MAX_BODY), "16 MiB");
}

#[test]
fn thinking_is_left_out_and_narration_kept() {
    let claude = json_lines_of(&[json!({"type": "assistant", "message": {"content": [{"type": "thinking", "thinking": "secret plan"}, {"type": "redacted_thinking", "data": "x"}, {"type": "text", "text": "answer"}]}})]);
    let out = render_vec(Agent::Claude, claude, MAX_BODY).text;
    assert!(!out.contains("secret plan") && out.contains("answer"));
    let omp = json_lines_of(&[json!({"type": "message", "message": {"role": "assistant", "content": [{"type": "thinking", "thinking": "inner"}, {"type": "thinking", "thinking": "said aloud", "narration": true}]}})]);
    let out = render_vec(Agent::Omp, omp, MAX_BODY).text;
    assert!(!out.contains("inner") && out.contains("said aloud"));
}

#[test]
fn writes_the_chosen_file_only_from_the_agents_folder() {
    let dir = scratch("write");
    let (claude_home, codex_home) = (dir.join("claude"), dir.join("codex"));
    let (cid, cwd) = (CASES[0].2, "D:\\work\\demo");
    let project = claude_project_dir(&claude_home, cwd);
    std::fs::create_dir_all(&project).unwrap();
    let transcript = project.join(format!("{cid}.jsonl"));
    std::fs::copy(fixtures().join("claude.jsonl"), &transcript).unwrap();
    let out = dir.join("out.md");
    let (claude_home_s, out_s) = (claude_home.to_string_lossy().into_owned(), out.to_string_lossy().into_owned());
    // Overwrites an earlier file, leaves no temporary file behind.
    std::fs::write(&out, "old").unwrap();
    let r = export_to("claude", cid, Some(cwd), Some(&claude_home_s), Some(CASES[0].3), &out_s).unwrap();
    assert_eq!(r, Exported { path: out_s.clone(), skipped: 1, truncated: false });
    assert_eq!(std::fs::read_to_string(&out).unwrap(), std::fs::read_to_string(fixtures().join("claude.md")).unwrap());
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(!names.iter().any(|n| n.ends_with(".tmp")), "{names:?}");
    // Refused: an id that is not one, a relative path, the transcript itself, an agent without readable history.
    assert!(export_to("claude", "../x", Some(cwd), Some(&claude_home_s), None, &out_s).is_err());
    assert!(export_to("claude", cid, Some(cwd), Some(&claude_home_s), None, "out.md").is_err());
    assert!(export_to("claude", cid, Some(cwd), Some(&claude_home_s), None, &transcript.to_string_lossy()).is_err());
    assert!(export_to("gemini", cid, Some(cwd), None, None, &out_s).is_err());
    assert!(export_to("claude", "no-such-conversation", Some(cwd), Some(&claude_home_s), None, &out_s).is_err());
    // A compressed Codex rollout is found and read like a plain one.
    let (xname, _, xid, xtitle) = CASES[1];
    let day = codex_home.join("sessions").join("2026").join("10").join("24");
    std::fs::create_dir_all(&day).unwrap();
    let plain = std::fs::read(fixtures().join(format!("{xname}.jsonl"))).unwrap();
    std::fs::write(day.join(format!("rollout-2026-10-24T02-00-00-{xid}.jsonl.zst")), zstd::encode_all(&plain[..], 3).unwrap()).unwrap();
    let xout = dir.join("codex.md");
    export_to("codex", xid, None, Some(&codex_home.to_string_lossy()), Some(xtitle), &xout.to_string_lossy()).unwrap();
    assert_eq!(std::fs::read_to_string(&xout).unwrap(), std::fs::read_to_string(fixtures().join("codex.md")).unwrap());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn secrets_on_later_lines_of_structured_tool_text_are_masked() {
    let (gh, sk, pw, url_pw, aws) = (SECRETS[0], SECRETS[1], SECRETS[5], SECRETS[3], SECRETS[4]);
    let env_file = format!("# local\nPORT=8080\nDB_PASSWORD={pw}\nGITHUB_TOKEN={gh}\n");
    let heredoc = format!("cat <<EOF > deploy.sh\nexport API_KEY={sk}\ncurl https://admin:{url_pw}@ci.local/x\nEOF");
    let json_out = json!({"vars": format!("A=1\nAWS_SECRET_ACCESS_KEY={aws}"), "nested": [{"token": gh}]}).to_string();
    let claude = json_lines_of(&[
        json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "w", "name": "Write", "input": {"file_path": ".env", "content": env_file}},
            {"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": heredoc}},
            {"type": "tool_use", "id": "m", "name": "mcp__vault__read", "input": {}},
        ]}}),
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "m", "content": [{"type": "text", "text": json_out}]}]}}),
    ]);
    let codex = json_lines_of(&[
        json!({"type": "response_item", "payload": {"type": "function_call", "name": "shell", "call_id": "c", "arguments": json!({"command": ["bash", "-lc", format!("set -e\nexport GITHUB_TOKEN={gh}\nDB_PASSWORD={pw} ./migrate")]}).to_string()}}),
        json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "c", "output": json_out}}),
    ]);
    let omp = json_lines_of(&[
        json!({"type": "message", "message": {"role": "assistant", "content": [{"type": "toolCall", "id": "e", "name": "edit", "arguments": {"path": ".env", "edits": [{"old": env_file, "new": "PORT=8080\n"}]}}]}}),
        json!({"type": "message", "message": {"role": "toolResult", "toolCallId": "e", "toolName": "edit", "content": [{"type": "text", "text": json_out}]}}),
    ]);
    for (agent, lines) in [(Agent::Claude, claude), (Agent::Codex, codex), (Agent::Omp, omp)] {
        let out = render_vec(agent, lines, MAX_BODY).text;
        for s in [gh, sk, pw, url_pw, aws] {
            assert!(!out.contains(s), "{agent:?}: {s} leaked:\n{out}");
        }
        assert!(out.contains("```json"), "{agent:?}: JSON output shown as JSON:\n{out}");
    }
    // Redacting the leaves first is what catches them: the serialised form alone hides them.
    let input = json!({ "content": format!("# keys\n{gh}\n") });
    assert!(redact(&serde_json::to_string_pretty(&input).unwrap()).contains(gh), "the escaped form is the case this guards against");
    assert!(!serde_json::to_string_pretty(&redact_value(&input)).unwrap().contains(gh));
}

#[test]
fn only_the_start_of_a_long_tool_output_is_redacted_and_never_half_a_secret() {
    let gh = SECRETS[0];
    // A secret straddling the character cut is masked whole: the window reaches past the cut.
    let text = format!("{} {gh} tail", "a".repeat(CUT_CHARS - 20));
    let out = tool_text(&text);
    assert!(!out.contains("ghp_FAKE") && out.contains("[redacted]"), "{out}");
    // Many lines: what follows the window is counted, not redacted or kept.
    let many = (0..5000).map(|i| format!("line {i} GITHUB_TOKEN={gh}")).collect::<Vec<_>>().join("\n");
    let (head, after) = window(&many);
    assert_eq!((head.lines().count(), after), (CUT_LINES + 1, 5000 - CUT_LINES - 1));
    let out = tool_text(&many);
    assert!(!out.contains(gh) && out.ends_with(&format!("… {} lines omitted", 5000 - CUT_LINES)), "{out}");
    // One giant line: the window stops at the cap plus the margin.
    let giant = "b".repeat(1 << 20);
    assert_eq!(window(&giant).0.len(), CUT_CHARS + REDACT_MARGIN);
}

#[test]
fn messages_cannot_break_the_document() {
    let opened = contain("see:\n<details>\n<summary>x</summary>\n  <!-- hidden\n</details>\n```yaml\nport: 1\n<details>");
    assert_eq!(opened, "see:\n\\<details>\n\\<summary>x</summary>\n  \\<!-- hidden\n\\</details>\n```yaml\nport: 1\n<details>\n```");
    // Closed fences, tildes, inline backticks and a `<` inside a line are left alone.
    assert_eq!(contain("~~~~\n<x>\n~~~~\nif a < b {}\n```inline``` text\n-<x> \n*<b>*"), "~~~~\n<x>\n~~~~\nif a < b {}\n```inline``` text\n-<x> \n*<b>*");
    // Behind blockquote and list markers, nested, or in a list item's indented continuation.
    let nested = "> quoted mail\n> <!-- tracking pixel\n- item\n  <textarea>\n- outer\n  - inner\n    <script>x</script>\n>> <details>\n1. > <style>\n2) <pre>\n- x\n\n    <!-- deep";
    let want = "> quoted mail\n> \\<!-- tracking pixel\n- item\n  \\<textarea>\n- outer\n  - inner\n    \\<script>x</script>\n>> \\<details>\n1. > \\<style>\n2) \\<pre>\n- x\n\n    \\<!-- deep";
    assert_eq!(contain(nested), want);
    assert_eq!(after_containers(" > - 12. >\t<x>"), "<x>");
    assert_eq!(after_containers("1234567890. <x>"), "1234567890. <x>", "ten digits are not a list marker");
    assert_eq!(contain("````md\n```\nstill code"), "````md\n```\nstill code\n````");
    // Each message is contained on its own: headings after an unclosed fence stay headings.
    let lines = json_lines_of(&[
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "fix:\n```diff\n- a"}]}}),
        json!({"type": "user", "message": {"content": "thanks"}}),
    ]);
    let out = render_vec(Agent::Claude, lines, MAX_BODY).text;
    assert!(out.contains("- a\n```\n\n## User\n\nthanks"), "{out}");
}

#[test]
fn an_omp_export_follows_the_active_branch() {
    let (name, agent, id, title) = CASES[2];
    let raw = std::fs::read_to_string(fixtures().join(format!("{name}.jsonl"))).unwrap();
    assert!(raw.contains("Abandoned"), "the fixture has a branch that was left");
    let out = export_fixture(name, agent, id, title).text;
    assert!(!out.contains("Abandoned") && out.contains("The logger now masks headers."), "{out}");
    assert!(out.contains("- **Ended:** 2026-10-24T03:02:00.000Z"), "the end is the last exported entry");
    // Without parent links (older sessions) everything is exported in file order.
    let mut flat = json_lines_of(&[json!({"type": "message", "id": "x", "message": {"role": "user", "content": "one"}}), json!({"type": "message", "id": "y", "message": {"role": "user", "content": "two"}})]).into_iter();
    assert!(active_branch(|| flat.next()).is_none());
    // A parent loop ends the walk.
    let mut looped = json_lines_of(&[json!({"id": "p", "parentId": "q"}), json!({"id": "q", "parentId": "p"})]).into_iter();
    assert_eq!(active_branch(|| looped.next()).map(|s| s.len()), Some(2));
}

#[test]
fn ids_homes_and_targets_that_could_reach_elsewhere_are_refused() {
    use crate::agent_history::file_id_ok;
    assert!(file_id_ok(CASES[0].2) && file_id_ok(CASES[2].2));
    for bad in ["C:x", "a:b", "a..b", "../x", "x/y", "x\\y", "", "-x"] {
        assert!(!file_id_ok(bad), "{bad}");
    }
    assert!(resolve_in(Agent::Claude, Path::new("D:\\nowhere"), "C:notes", None, None).is_none());
    let out = std::env::temp_dir().join("kelpie-never-written.md").to_string_lossy().into_owned();
    for (home, target) in [(Some("\\\\server\\share\\.claude"), out.as_str()), (None, "\\\\.\\pipe\\kelpie-test"), (None, "//./pipe/kelpie-test")] {
        assert!(export_to("claude", CASES[0].2, None, home, None, target).is_err(), "{home:?} {target}");
    }
}

#[test]
fn a_failed_write_leaves_no_temporary_file() {
    let dir = scratch("failed-write");
    // The target is a folder: refused before anything is created.
    assert!(crate::config_transfer::write_file_atomic(&dir, b"x").is_err());
    // The temporary file cannot be created: the folder is missing.
    assert!(crate::config_transfer::write_file_atomic(&dir.join("missing").join("out.md"), b"x").is_err());
    // Two writes to one target each use their own temporary file.
    let target = dir.join("out.md");
    std::thread::scope(|s| {
        for i in 0..4 {
            let target = &target;
            s.spawn(move || crate::config_transfer::write_file_atomic(target, format!("writer {i}\n").repeat(1000).as_bytes()).unwrap());
        }
    });
    let text = std::fs::read_to_string(&target).unwrap();
    assert!(text.lines().all(|l| l == text.lines().next().unwrap()), "one writer's text, whole");
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).filter(|n| n.to_string_lossy().ends_with(".tmp")).collect();
    assert!(left.is_empty(), "{left:?}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn numbers_json_lines_and_escaped_json_are_redacted_too() {
    let (gh, pw) = (SECRETS[0], SECRETS[5]);
    // A number under a secret-named key.
    let v = redact_value(&json!({"password": 12345678, "port": 8080, "db": {"api_key": 42}}));
    assert_eq!(v, json!({"password": "[redacted]", "port": 8080, "db": {"api_key": "[redacted]"}}));
    assert!(secret_name("DB_PASSWORD") && secret_name("token") && !secret_name("port") && !secret_name("tokenizer"));
    // JSON lines: each line's strings, kept one per line; other lines untouched.
    let ndjson = format!("start\n{}\n{}", json!({"log": format!("x\n{gh}")}), json!({"env": format!("A=1\nDB_PASSWORD={pw}"), "n": 1}));
    let out = json_lines(&ndjson).unwrap();
    assert!(!out.contains(gh) && !out.contains(pw) && out.starts_with("start\n{") && out.lines().count() == 3, "{out}");
    assert!(json_lines("plain\ntext").is_none());
    // JSON after a text prefix (not whole): its escapes are undone for redaction.
    let prefixed = format!("Response:\n{}", json!({"vars": format!("A=1\nAWS_SECRET_ACCESS_KEY={}", SECRETS[4]), "k": format!("x\n{gh}")}));
    assert!(redact(&prefixed).contains(gh), "the escaped form is the case this guards against");
    let out = tool_text(&prefixed);
    assert!(!out.contains(gh) && !out.contains(SECRETS[4]), "{out}");
    // Source code with `\n` in a string literal and nothing to mask keeps its escapes.
    assert_eq!(redact_escaped(r#"printf("a\nb\t\"c\"");"#), r#"printf("a\nb\t\"c\"");"#);
    // Through a whole export: an NDJSON tool output and a numeric PIN in a tool input.
    let lines = json_lines_of(&[
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t", "name": "Bash", "input": {"command": "tail log.jsonl", "pin_password": 98765432}}]}}),
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t", "content": ndjson}]}}),
    ]);
    let out = render_vec(Agent::Claude, lines, MAX_BODY).text;
    assert!(!out.contains(gh) && !out.contains(pw) && !out.contains("98765432"), "{out}");
}

#[test]
fn a_broken_omp_chain_falls_back_to_file_order() {
    let first = json!({"type": "message", "id": "e1", "parentId": null, "message": {"role": "user", "content": "first"}}).to_string();
    let cut = r#"{"type":"message","id":"e2","parentId":"e1","message":"#.to_string();
    let last = json!({"type": "message", "id": "e3", "parentId": "e2", "message": {"role": "user", "content": "last"}}).to_string();
    // e2's line could not be read: the walk from e3 cannot reach the start.
    let mut lines = vec![Ok(first.clone()), Ok(cut.clone()), Ok(last.clone())].into_iter();
    assert!(active_branch(|| lines.next()).is_none());
    // A leaf an off-branch entry names that is not in the file: file order too.
    let mut lines = json_lines_of(&[json!({"id": "a", "parentId": null}), json!({"id": "b", "parentId": "a", "activeLeafId": "zz"})]).into_iter();
    assert!(active_branch(|| lines.next()).is_none());
    // Through `render`: the first message is kept.
    let dir = scratch("broken-chain");
    let path = dir.join("s.jsonl");
    std::fs::write(&path, format!("{first}\n{cut}\n{last}\n")).unwrap();
    let out = render(Agent::Omp, &path, &Request { id: "s", cwd: None, title: None }).unwrap().text;
    assert!(out.contains("first") && out.contains("last"), "{out}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_write_that_fails_after_its_temporary_file_exists_removes_it() {
    let dir = scratch("late-failure");
    let target = dir.join("out.md");
    std::fs::write(&target, "kept").unwrap();
    let seen = std::sync::Mutex::new(None);
    let failing = |from: &Path, _: &Path| {
        *seen.lock().unwrap() = Some((from.to_path_buf(), from.exists()));
        Err(std::io::Error::other("rename refused"))
    };
    assert!(crate::config_transfer::write_atomic_with(&target, b"new text", failing).is_err());
    let (tmp, existed) = seen.lock().unwrap().clone().unwrap();
    assert!(existed && !tmp.exists(), "the temporary file existed and is gone");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "kept", "the target is untouched");
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).filter(|n| n.to_string_lossy().ends_with(".tmp")).collect();
    assert!(left.is_empty(), "{left:?}");
    std::fs::remove_dir_all(&dir).ok();
}
