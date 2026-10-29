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
    render_lines(agent, || it.next(), &Request { id: "t1", cwd: Some("/w"), title: None }, max_body)
}

fn json_lines(values: &[Value]) -> Vec<Result<String, u64>> {
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
    let claude = json_lines(&[
        json!({"type": "user", "message": {"role": "user", "content": format!("user text {gh}")}}),
        json!({"type": "assistant", "message": {"role": "assistant", "content": [
            {"type": "text", "text": format!("assistant text {sk}")},
            {"type": "tool_use", "id": "t", "name": "Bash", "input": {"command": format!("DB_PASSWORD={pw} ./deploy"), "password": pw}},
        ]}}),
        json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t", "content": format!("tool output Authorization: Bearer {bearer}")}]}}),
    ]);
    let codex = json_lines(&[
        json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": format!("user text {gh}")}]}}),
        json!({"type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": format!("assistant text {sk}")}]}}),
        json!({"type": "response_item", "payload": {"type": "function_call", "name": "shell", "call_id": "c", "arguments": json!({"command": ["bash", "-lc", format!("export API_KEY={pw}")]}).to_string()}}),
        json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "c", "output": format!("tool output Authorization: Bearer {bearer}")}}),
    ]);
    let omp = json_lines(&[
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
    let out = cut(&many);
    assert!(out.starts_with("line 1\n") && out.ends_with("line 40\n… 60 lines omitted"), "{out}");
    assert_eq!(cut("short\ntext"), "short\ntext");
    // The character allowance stops before the line cap: 200-character lines, 19 fit in 4000.
    let wide = (0..30).map(|_| "w".repeat(200)).collect::<Vec<_>>().join("\n");
    assert!(cut(&wide).ends_with("\n… 11 lines omitted"));
    // One huge line keeps its start.
    let huge = "x".repeat(10_000);
    assert_eq!(cut(&huge), format!("{}…", "x".repeat(CUT_CHARS)));
    assert_eq!(cut(&format!("{huge}\nnext")), format!("{}…\n… 1 line omitted", "x".repeat(CUT_CHARS)));
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
    let r = render_vec(Agent::Claude, json_lines(&(0..20).map(line).collect::<Vec<_>>()), 2000);
    assert!(r.truncated);
    assert!(r.text.contains("reply 3 ") && !r.text.contains("reply 4 "), "{}", r.text);
    assert!(r.text.ends_with("*The export stops here: the conversation is longer than 2000 bytes of Markdown.*\n"));
    assert_eq!(size(MAX_BODY), "16 MiB");
}

#[test]
fn thinking_is_left_out_and_narration_kept() {
    let claude = json_lines(&[json!({"type": "assistant", "message": {"content": [{"type": "thinking", "thinking": "secret plan"}, {"type": "redacted_thinking", "data": "x"}, {"type": "text", "text": "answer"}]}})]);
    let out = render_vec(Agent::Claude, claude, MAX_BODY).text;
    assert!(!out.contains("secret plan") && out.contains("answer"));
    let omp = json_lines(&[json!({"type": "message", "message": {"role": "assistant", "content": [{"type": "thinking", "thinking": "inner"}, {"type": "thinking", "thinking": "said aloud", "narration": true}]}})]);
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
