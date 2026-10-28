//! Masks credentials in text Kelpie sends out of a session: hand-offs, Copy Context, review
//! notes and issues sent to an agent, notifications. Recognisable key formats are replaced
//! whole; `NAME=value` lines and URLs keep their name / host and lose only the secret part.
//! Best effort: it catches the common shapes, not every secret.

use std::sync::LazyLock;

use regex::Regex;

pub const MASK: &str = "[redacted]";

/// Whole-match patterns: well-known token formats.
static TOKENS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // PEM / PGP private keys, across lines; a block cut off by a line limit (Copy Context
        // keeps the last 400 lines) is masked to the end, or from its first base64 line.
        r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----(?:[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|[\s\S]*$)",
        r"(?m)(?:^[A-Za-z0-9+/=]{40,}\r?\n)+-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----",
        // Stripe, npm, Hugging Face.
        r"\b[rs]k_(?:live|test)_[A-Za-z0-9]{16,}",
        r"\bnpm_[A-Za-z0-9]{36}\b",
        r"\bhf_[A-Za-z0-9]{30,}\b",
        // AWS access key ids.
        r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        // GitHub tokens.
        r"\bgh[pousr]_[A-Za-z0-9]{36,}\b",
        r"\bgithub_pat_[A-Za-z0-9_]{22,}\b",
        // OpenAI / Anthropic style keys (sk-…, sk-proj-…, sk-ant-…).
        r"\bsk-[A-Za-z0-9_-]{20,}",
        // Slack tokens.
        r"\bxox[abprs]-[A-Za-z0-9-]{10,}",
        // Google API keys.
        r"\bAIza[0-9A-Za-z_-]{35}\b",
        // GitLab personal access tokens.
        r"\bglpat-[A-Za-z0-9_-]{20,}",
        // JSON Web Tokens.
        r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("redaction pattern"))
    .collect()
});

/// Names that hold secrets: they end in the secret word (`GITHUB_TOKEN`, not `tokenizer`).
const SECRET_NAME: &str = r"(?:SECRET|TOKEN|PASSW(?:OR)?D|PWD|API_?KEY|ACCESS_?KEY|PRIVATE_?KEY|SECRET_?KEY|CREDENTIALS?)";

/// Values that look like secrets: no spaces, quotes or code punctuation, 6+ characters.
const SECRET_VALUE: &str = r#"[^\s"'`,;(){}\[\]&<>:=]{6,}"#;

/// Where a secret's value sits, keeping its name: names and values must look like
/// configuration, not code (`pub password: String`, `let token = getToken()` stay).
static ASSIGNMENTS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // Environment style: `GITHUB_TOKEN=…`, `export DB_PASSWORD="…"` (upper-case names).
        format!(r#"((?:^|[^A-Za-z0-9_])[A-Z0-9_]*{SECRET_NAME}\s*=\s*["']?)({SECRET_VALUE})"#),
        // A quoted value of a secret-named key: `"api_key": "…"`, `password = '…'`, `token: "…"`.
        format!(r#"(?i)((?:^|[^A-Za-z0-9_])[A-Za-z0-9_.-]*{SECRET_NAME}["']?\s*[:=]\s*["'])({SECRET_VALUE})"#),
        // YAML / INI without quotes: `password: hunter2x` alone on its line, with a digit.
        format!(r#"(?im)(^\s*[A-Za-z0-9_.-]*{SECRET_NAME}\s*[:=]\s*)((?:[^\s"'`,;(){{}}\[\]&<>:=]*\d)[^\s"'`,;(){{}}\[\]&<>:=]*)\s*$"#),
    ]
    .iter()
    .map(|p| Regex::new(p).expect("assignment pattern"))
    .collect()
});

/// `Authorization: Bearer …`.
static BEARER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\bbearer\s+)[A-Za-z0-9._~+/-]{16,}=*").expect("bearer pattern"));

/// Passwords in URLs: `scheme://user:password@host`.
static URL_PASSWORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\b[a-z][a-z0-9+.-]*://[^\s:/@]+:)([^\s@/]{3,})(@)").expect("url pattern"));

/// `text` with recognisable secrets replaced by [`MASK`].
pub fn redact(text: &str) -> String {
    let mut out = text.to_string();
    for re in TOKENS.iter() {
        if re.is_match(&out) {
            out = re.replace_all(&out, MASK).into_owned();
        }
    }
    out = URL_PASSWORD.replace_all(&out, format!("${{1}}{MASK}${{3}}")).into_owned();
    out = BEARER.replace_all(&out, format!("${{1}}{MASK}")).into_owned();
    for re in ASSIGNMENTS.iter() {
        out = re.replace_all(&out, |c: &regex::Captures| format!("{}{MASK}", &c[1])).into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_known_token_formats() {
        let cases = [
            "AKIAIOSFODNN7EXAMPLE",
            "ghp_0123456789abcdefghijABCDEFGHIJ012345",
            "github_pat_11ABCDEFG0123456789_abcdefghijklmnop",
            "sk-proj-abcdefghijklmnopqrstuvwxyz012345",
            "sk-ant-api03-abcdefghijklmnopqrstuvwxyz",
            "xoxb-1234567890-abcdefghij",
            "AIzaSyA-1234567890abcdefghijklmnopqrstu",
            "glpat-abcdefghijklmnopqrst",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
        ];
        for c in cases {
            let out = redact(&format!("value {c} end"));
            assert!(!out.contains(c), "{c} leaked: {out}");
            assert!(out.contains(MASK), "{c}: {out}");
        }
    }

    #[test]
    fn masks_private_key_blocks() {
        let pem = "a\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nAAAA\n-----END OPENSSH PRIVATE KEY-----\nz";
        assert_eq!(redact(pem), format!("a\n{MASK}\nz"));
    }

    #[test]
    fn keeps_names_and_hosts() {
        assert_eq!(redact("DB_PASSWORD=hunter2hunter2"), format!("DB_PASSWORD={MASK}"));
        assert_eq!(redact("export GITHUB_TOKEN=\"abcdef123456\""), format!("export GITHUB_TOKEN=\"{MASK}\""));
        assert_eq!(redact(r#"{"api_key": "abcdef123456"}"#), format!(r#"{{"api_key": "{MASK}"}}"#));
        assert_eq!(redact("Authorization: Bearer abcdefghijklmnop123456"), format!("Authorization: Bearer {MASK}"));
        assert_eq!(redact("postgres://app:s3cr3tpw@db.local:5432/x"), format!("postgres://app:{MASK}@db.local:5432/x"));
    }

    #[test]
    fn masks_more_formats_and_cut_key_blocks() {
        for c in ["sk_live_abcdefghijklmnop1234", "rk_test_ABCDEFGHIJKLMNOP5678", "npm_abcdefghijklmnopqrstuvwxyz0123456789", "hf_abcdefghijklmnopqrstuvwxyzABCDEF"] {
            let out = redact(&format!("x {c} y"));
            assert!(!out.contains(c), "{c} leaked: {out}");
        }
        let pgp = "-----BEGIN PGP PRIVATE KEY BLOCK-----\nlQOYBF\n-----END PGP PRIVATE KEY BLOCK-----";
        assert_eq!(redact(pgp), MASK);
        // Copy Context cut the block: no END, or no BEGIN.
        assert_eq!(redact("ok\n-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\nmore"), format!("ok\n{MASK}"));
        let tail = "ok\nMIIEpAIBAAKCAQEAw1qX9Zy0123456789abcdefABCDEF0123456789+/abcd\nAAAAB3NzaC1yc2EAAAADAQABAAABAQDx0123456789abcdefghijklmnopq==\n-----END RSA PRIVATE KEY-----\nz";
        assert_eq!(redact(tail), format!("ok\n{MASK}\nz"));
    }

    /// `cargo test -p kelpie-proto --release redact_speed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn redact_speed() {
        let line = "    let total = items.iter().map(|x| x.price * x.qty).sum::<f64>(); // GITHUB_TOKEN=ghp_0123456789abcdefghijABCDEFGHIJ012345\n";
        let text = line.repeat(64 * 1024 / line.len());
        redact(&text);
        let start = std::time::Instant::now();
        for _ in 0..20 {
            redact(&text);
        }
        println!("redact {} KiB: {:.2} ms", text.len() / 1024, start.elapsed().as_secs_f64() * 1000.0 / 20.0);
    }

    #[test]
    fn masks_config_values() {
        assert_eq!(redact("password: hunter2x"), format!("password: {MASK}"));
        assert_eq!(redact("  db_password = s3cretvalue"), format!("  db_password = {MASK}"));
        assert_eq!(redact("password = 'hunter2hunter'"), format!("password = '{MASK}'"));
    }

    #[test]
    fn leaves_ordinary_code_alone() {
        let code = "fn token_count(tokens: &[Token]) -> usize { tokens.len() }\nlet password_len = 12;\nconst MAX = 5;\nhttps://example.com/path?q=1\nsk-short";
        assert_eq!(redact(code), code);
        for line in [
            "let tokenizer = Tokenizer::new();",
            "pub password: String,",
            "    password: String",
            "let token = getToken();",
            "api_key: Option<String>,",
            "const token = await getToken(user)",
            "if (password === confirmPassword) {",
            "self.api_key = api_key",
            "password: str",
            "TOKEN_TTL_SECONDS = 3600",
        ] {
            assert_eq!(redact(line), line, "mangled: {line}");
        }
    }
}
