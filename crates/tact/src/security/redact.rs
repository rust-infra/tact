//! Redaction of secrets from tool output.
//!
//! The sensitive-path guard in [`super::sensitive`] is a name-based heuristic
//! and is bypassable by design — `python -c "print(open('/home/me/.ssh/id_ed25519').read())"`
//! names no path the tokenizer can see. Redaction is the part that still holds
//! when the guard is walked past, and it is the last thing between a secret and
//! the transcript, the session store, and the terminal.
//!
//! # Two levels
//!
//! Blanket-applying key/value rules to every tool result would rewrite the
//! user's own source code and test fixtures — the model would then be reasoning
//! about `[redacted:value]` where the file says `token = "abc"`. So:
//!
//! - [`RedactionLevel::Basic`] — high-confidence, low-false-positive shapes
//!   (private-key blocks, `sk-…`, `AKIA…`, JWTs, `user:pass@` URLs). Safe to run
//!   over a Rust file or a diff, and therefore run over **everything**.
//! - [`RedactionLevel::Credential`] — [`RedactionLevel::Basic`] plus the
//!   structure-aware rules that only make sense for text already known to be a
//!   credential store (`.netrc` `password …`, dotenv/INI `API_KEY=…`, JSON
//!   `"token": "…"`). Run over a call the guard classified sensitive.
//!
//! # What is not redacted
//!
//! Tool *use* inputs and `arg_full`. The model's own `tool_use` block has to
//! round-trip byte-identical or the next request is malformed, and if the model
//! emitted a secret then redacting the echo does not un-leak it. Only tool
//! *results* and command output go through here.

use std::borrow::Cow;
use std::sync::OnceLock;

use regex::Regex;

use super::{RedactionConfig, RedactionLevel};

/// A compiled rule: which level it belongs to, the pattern, and the replacement.
struct Rule {
    credential_only: bool,
    regex: Regex,
    replacement: &'static str,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        // (credential_only, pattern, replacement)
        let specs: &[(bool, &str, &str)] = &[
            // ── Basic: private key material ────────────────────────────────
            (
                false,
                r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
                "[redacted:private-key]",
            ),
            (
                false,
                r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY BLOCK-----.*?-----END [A-Z0-9 ]*PRIVATE KEY BLOCK-----",
                "[redacted:private-key]",
            ),
            // ── Basic: provider tokens ─────────────────────────────────────
            // Covers `sk-ant-…` too, which is `sk-` followed by a long body.
            (false, r"\bsk-[A-Za-z0-9_-]{20,}", "[redacted:api-key]"),
            (false, r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", "[redacted:aws-key]"),
            (false, r"\bgh[pousr]_[A-Za-z0-9]{20,}\b", "[redacted:github-token]"),
            (
                false,
                r"\bgithub_pat_[A-Za-z0-9_]{22,}\b",
                "[redacted:github-token]",
            ),
            (false, r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b", "[redacted:slack-token]"),
            (false, r"\bglpat-[A-Za-z0-9_-]{20,}\b", "[redacted:gitlab-token]"),
            (false, r"\bAIza[0-9A-Za-z_-]{35}\b", "[redacted:google-key]"),
            (
                false,
                r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
                "[redacted:jwt]",
            ),
            (
                false,
                r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]{20,}",
                "[redacted:bearer]",
            ),
            (
                false,
                r"(?i)\bhttps?://[^/\s:@]+:[^/\s:@]+@",
                "[redacted:basic-auth]",
            ),
            // ── Credential: structure-aware key/value ──────────────────────
            // `.netrc`, both the full `machine … login … password …` line and a
            // bare `password …` continuation line.
            (
                true,
                r"(?m)^([ \t]*machine\s+\S+\s+login\s+\S+\s+)password\s+\S+",
                "${1}password [redacted:value]",
            ),
            (
                true,
                r"(?m)^([ \t]*)password\s+\S+",
                "${1}password [redacted:value]",
            ),
            // dotenv / INI / TOML. The key is kept so the shape survives.
            (
                true,
                r"(?mi)^([ \t]*[A-Z0-9_]*(?:PASSWORD|PASSWD|SECRET|TOKEN|API_?KEY|ACCESS_?KEY|PRIVATE_?KEY|CREDENTIAL|AUTH)[A-Z0-9_]*[ \t]*[:=][ \t]*).*$",
                "${1}[redacted:value]",
            ),
            // JSON.
            (
                true,
                r#"(?i)("[^"]*(?:password|secret|token|api_?key|auth|credential)[^"]*"[ \t]*:[ \t]*")[^"]*(")"#,
                "${1}[redacted:value]${2}",
            ),
            // npmrc.
            (
                true,
                r"(?m)^([ \t]*//\S+:_authToken=).*$",
                "${1}[redacted:value]",
            ),
            // `authorized_keys`.
            (
                true,
                r"(?m)^(ssh-[a-z0-9-]+\s+)[A-Za-z0-9+/=]{20,}",
                "${1}[redacted:authorized-key]",
            ),
        ];

        specs
            .iter()
            .map(|(credential_only, pattern, replacement)| {
                // A malformed built-in is a programming error, not a runtime
                // condition: panicking keeps it loud instead of silently
                // shipping a rule that never fires.
                let regex = Regex::new(pattern).unwrap_or_else(|e| {
                    panic!("built-in redaction pattern {pattern:?} does not compile: {e}")
                });
                Rule {
                    credential_only: *credential_only,
                    regex,
                    replacement,
                }
            })
            .collect()
    })
}

/// Extra patterns from `permissions.redaction.extra_patterns`.
///
/// Compiled per call rather than cached because the set is
/// user-configuration-sized (a handful), and a cache would have to be keyed on
/// it — which is more machinery than the compile costs. An invalid pattern is
/// skipped with a warning: a typo in a security config must not break tool
/// dispatch.
fn compile_extra(patterns: &[String]) -> Vec<Regex> {
    patterns
        .iter()
        .filter_map(|p| match Regex::new(p) {
            Ok(re) => Some(re),
            Err(e) => {
                tracing::warn!(
                    "Ignoring invalid permissions.redaction.extra_patterns entry {p:?}: {e}"
                );
                None
            }
        })
        .collect()
}

/// Redact `text` at `level`.
///
/// Returns [`Cow::Borrowed`] when nothing matched, so the common case (source
/// code, diffs, ordinary command output) costs no allocation.
#[must_use]
pub fn redact<'a>(text: &'a str, level: RedactionLevel, extra_patterns: &[String]) -> Cow<'a, str> {
    if level == RedactionLevel::Off || text.is_empty() {
        return Cow::Borrowed(text);
    }

    let mut out = Cow::Borrowed(text);
    for rule in rules() {
        if rule.credential_only && level != RedactionLevel::Credential {
            continue;
        }
        if let Cow::Owned(replaced) = rule.regex.replace_all(out.as_ref(), rule.replacement) {
            out = Cow::Owned(replaced);
        }
    }

    if !extra_patterns.is_empty() {
        for regex in compile_extra(extra_patterns) {
            if let Cow::Owned(replaced) = regex.replace_all(out.as_ref(), "[redacted]") {
                out = Cow::Owned(replaced);
            }
        }
    }

    out
}

/// The level one call's result should be redacted at.
///
/// `Basic` is the floor for everything. `Credential` is the *default level's*
/// upgrade for a call the guard classified sensitive — that is what "the
/// structural rules apply to credential stores" means in practice — and the
/// explicit `level: "credential"` setting promotes every call.
///
/// `basic_only_paths` then caps it back down: a call whose target is a fixture
/// or a source tree keeps the high-confidence rules and is spared the
/// structure-aware ones, which would otherwise rewrite the user's own code.
#[must_use]
pub fn level_for_call(
    config: &RedactionConfig,
    call_is_sensitive: bool,
    target: Option<&str>,
) -> RedactionLevel {
    if !config.is_enabled() {
        return RedactionLevel::Off;
    }
    let level = match config.resolved_level() {
        RedactionLevel::Off => RedactionLevel::Off,
        RedactionLevel::Credential => RedactionLevel::Credential,
        RedactionLevel::Basic if call_is_sensitive => RedactionLevel::Credential,
        RedactionLevel::Basic => RedactionLevel::Basic,
    };
    if level == RedactionLevel::Credential
        && target.is_some_and(|t| super::sensitive::matches_any(&config.basic_only_paths, t))
    {
        return RedactionLevel::Basic;
    }
    level
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Longest run of held-back text before it is emitted regardless.
///
/// A line with no newline in sight is not a shape any rule matches, and holding
/// it forever would stall a `yes`-style command's output on screen.
const MAX_HOLDBACK: usize = 64 * 1024;

const PEM_BEGIN: &str = "-----BEGIN ";
const PEM_END_PREFIX: &str = "-----END ";

/// Redacts a live output stream, one chunk at a time.
///
/// Command output arrives in arbitrary chunks, so a secret can straddle a
/// boundary. Two things make that safe:
///
/// - Only **complete lines** are emitted, so a pattern anchored within a line
///   (every `Basic` rule except the private-key block) always sees all of its
///   input before any of it is shown.
/// - A private-key block is multi-line and cannot be held line-by-line, so once
///   a `-----BEGIN … PRIVATE KEY` marker is seen, output is suppressed until the
///   matching `-----END`, and the whole block is replaced by the marker.
///
/// Only the high-confidence rules run here. The `Credential` rules are
/// line-anchored *and* need to see a whole line of context that a stream may not
/// have delivered yet; the final pass over the completed result is what
/// guarantees them.
#[derive(Debug, Clone)]
pub struct StreamRedactor {
    /// Only on/off: this redactor always runs the `Basic` rules, and the level
    /// is used solely to decide whether it runs at all.
    enabled: bool,
    holdback: String,
    in_private_key_block: bool,
}

impl StreamRedactor {
    #[must_use]
    pub fn new(level: RedactionLevel) -> Self {
        Self {
            enabled: level != RedactionLevel::Off,
            holdback: String::new(),
            in_private_key_block: false,
        }
    }

    /// Feed a chunk; returns the text that is now safe to display, if any.
    pub fn push(&mut self, chunk: &str) -> Option<String> {
        if !self.enabled {
            return (!chunk.is_empty()).then(|| chunk.to_string());
        }
        self.holdback.push_str(chunk);
        self.drain(false)
    }

    /// Flush whatever is held back. Must be called on **every** exit path —
    /// success, error and cancellation — or the tail of the output is lost.
    pub fn finish(&mut self) -> Option<String> {
        if !self.enabled {
            return None;
        }
        self.drain(true)
    }

    fn drain(&mut self, at_end: bool) -> Option<String> {
        if self.in_private_key_block {
            if let Some(idx) = find_private_key_end(&self.holdback) {
                let rest = self.holdback[idx..].to_string();
                self.holdback = rest;
                self.in_private_key_block = false;
                let mut out = String::from("[redacted:private-key]");
                if let Some(tail) = self.drain(at_end) {
                    out.push_str(&tail);
                }
                return Some(out);
            }
            // Still inside the block: show nothing, and drop what has already
            // been consumed so the buffer cannot grow without bound.
            if self.holdback.len() > MAX_HOLDBACK {
                self.holdback.clear();
            }
            return None;
        }

        if let Some(idx) = find_private_key_begin(&self.holdback) {
            // Emit everything before the block, then hold the block itself.
            let before = self.holdback[..idx].to_string();
            let from = self.holdback[idx..].to_string();
            self.holdback = from;
            self.in_private_key_block = true;
            let mut out = redact(&before, RedactionLevel::Basic, &[]).into_owned();
            if let Some(tail) = self.drain(at_end) {
                out.push_str(&tail);
            }
            return Some(out);
        }

        let split_at = if at_end {
            self.holdback.len()
        } else {
            match self.holdback.rfind('\n') {
                Some(pos) => pos + 1,
                None if self.holdback.len() > MAX_HOLDBACK => self.holdback.len(),
                None => return None,
            }
        };
        if split_at == 0 {
            return None;
        }
        let ready = self.holdback[..split_at].to_string();
        self.holdback = self.holdback[split_at..].to_string();
        Some(redact(&ready, RedactionLevel::Basic, &[]).into_owned())
    }
}

/// Index of a private-key block opening, if one is present.
///
/// Requires the closing dashes so a truncated `-----BEGIN ` at the end of a
/// chunk is *held* rather than rendered, which is the whole point of keeping a
/// buffer.
fn find_private_key_begin(s: &str) -> Option<usize> {
    let mut search = 0;
    while let Some(rel) = s[search..].find(PEM_BEGIN) {
        let at = search + rel;
        let line_end = s[at..].find('\n').map_or(s.len(), |e| at + e);
        let header = &s[at..line_end];
        if header.contains("PRIVATE KEY") && header.trim_end().ends_with("-----") {
            return Some(at);
        }
        search = at + PEM_BEGIN.len();
    }
    None
}

/// Index just past the end of a private-key block's closing line.
fn find_private_key_end(s: &str) -> Option<usize> {
    let mut search = 0;
    while let Some(rel) = s[search..].find(PEM_END_PREFIX) {
        let at = search + rel;
        let line_end = s[at..].find('\n').map_or(s.len(), |e| at + e);
        let header = &s[at..line_end];
        if header.contains("PRIVATE KEY") && header.trim_end().ends_with("-----") {
            return Some(if line_end < s.len() {
                line_end + 1
            } else {
                line_end
            });
        }
        search = at + PEM_END_PREFIX.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityConfig;

    fn basic(text: &str) -> String {
        redact(text, RedactionLevel::Basic, &[]).into_owned()
    }

    fn credential(text: &str) -> String {
        redact(text, RedactionLevel::Credential, &[]).into_owned()
    }

    #[test]
    fn basic_redacts_provider_tokens() {
        for (input, expected_marker) in [
            (
                "ANTHROPIC_AUTH_TOKEN=sk-7325c3231cef402d8481c32a49c4898a",
                "[redacted:api-key]",
            ),
            (
                "key = sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123",
                "[redacted:api-key]",
            ),
            (
                "aws_access_key_id = AKIAIOSFODNN7EXAMPLE",
                "[redacted:aws-key]",
            ),
            (
                "token: ghp_0123456789abcdefghijklmnopqrstuvwx",
                "[redacted:github-token]",
            ),
            ("xoxb-1234567890-abcdefghijkl", "[redacted:slack-token]"),
            ("glpat-abcdefghijklmnopqrst", "[redacted:gitlab-token]"),
            (
                "AIzaSyA1234567890abcdefghijklmnopqrstuv",
                "[redacted:google-key]",
            ),
            (
                "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk",
                "[redacted:jwt]",
            ),
            (
                "Authorization: Bearer abcdefghijklmnopqrstuvwxyz01",
                "[redacted:bearer]",
            ),
            (
                "https://alice:hunter2@example.com/repo.git",
                "[redacted:basic-auth]",
            ),
        ] {
            let out = basic(input);
            assert!(out.contains(expected_marker), "{input} → {out}");
            assert!(!out.contains("hunter2"), "{out}");
        }
    }

    #[test]
    fn basic_redacts_a_private_key_block() {
        let text = "before\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nAAAA\n-----END OPENSSH PRIVATE KEY-----\nafter\n";
        let out = basic(text);
        assert!(out.contains("[redacted:private-key]"), "{out}");
        assert!(!out.contains("b3BlbnNzaC1rZXk"), "{out}");
        assert!(out.contains("before"), "{out}");
        assert!(out.contains("after"), "{out}");
    }

    /// The whole reason the levels exist: a diff or a Rust file must survive.
    #[test]
    fn basic_does_not_touch_source_code() {
        for text in [
            "let token = compute(x);",
            "api_key: String,",
            "fn redact(text: &str) -> String {",
            "password_hash = argon2(password);",
            "pub struct CredentialStore {",
        ] {
            assert_eq!(basic(text), text, "false positive on: {text}");
        }
    }

    #[test]
    fn credential_redacts_netrc_and_env_files() {
        let netrc = "machine api.example.com\n  login alice\n  password s3cr3t-value\n";
        let out = credential(netrc);
        assert!(out.contains("password [redacted:value]"), "{out}");
        assert!(!out.contains("s3cr3t-value"), "{out}");

        let env = "DATABASE_URL=postgres://u:p@h/db\nAPI_KEY=abc123\nPORT=8080\n";
        let out = credential(env);
        assert!(out.contains("API_KEY=[redacted:value]"), "{out}");
        assert!(!out.contains("abc123"), "{out}");
        assert!(out.contains("PORT=8080"), "non-secret keys survive: {out}");
    }

    #[test]
    fn credential_redacts_json_npmrc_and_authorized_keys() {
        let json = r#"{"api_key": "abcdef123456", "region": "eu-west-1"}"#;
        let out = credential(json);
        assert!(out.contains(r#""api_key": "[redacted:value]""#), "{out}");
        assert!(out.contains("eu-west-1"), "{out}");

        let npmrc = "//registry.npmjs.org/:_authToken=npm_abcdef123456\n";
        let out = credential(npmrc);
        assert!(out.contains("_authToken=[redacted:value]"), "{out}");
        assert!(!out.contains("npm_abcdef123456"), "{out}");

        let keys = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIB1g2h3j4k5l6m7n8o9p0q user@host\n";
        let out = credential(keys);
        assert!(out.contains("[redacted:authorized-key]"), "{out}");
    }

    #[test]
    fn credential_rules_do_not_run_at_basic_level() {
        let env = "API_KEY=abc123\n";
        assert_eq!(basic(env), env, "the structural rules are opt-in per call");
    }

    #[test]
    fn redaction_is_idempotent() {
        let once = credential("API_KEY=abc123\n");
        assert_eq!(credential(&once), once);
    }

    #[test]
    fn off_redacts_nothing() {
        let text = "API_KEY=abc123 sk-7325c3231cef402d8481c32a49c4898a";
        assert_eq!(redact(text, RedactionLevel::Off, &[]), text);
    }

    #[test]
    fn borrowed_when_nothing_matches() {
        assert!(matches!(
            redact("nothing to see", RedactionLevel::Basic, &[]),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn extra_patterns_are_applied() {
        let out = redact(
            "token MYCO-0123456789abcdef0123456789abcdef",
            RedactionLevel::Basic,
            &["MYCO-[0-9a-f]{32}".to_string()],
        );
        assert!(out.contains("[redacted]"), "{out}");
        assert!(!out.contains("MYCO-0123"), "{out}");
    }

    #[test]
    fn an_invalid_extra_pattern_is_skipped_not_fatal() {
        let out = redact(
            "sk-7325c3231cef402d8481c32a49c4898a",
            RedactionLevel::Basic,
            &["this is not (a regex".to_string()],
        );
        assert!(out.contains("[redacted:api-key]"), "{out}");
    }

    // ── level_for_call ──────────────────────────────────────────────────

    fn config(json: serde_json::Value) -> RedactionConfig {
        SecurityConfig::from_document(&json).redaction
    }

    #[test]
    fn basic_is_the_floor_and_a_sensitive_call_gets_the_structural_rules() {
        let cfg = config(serde_json::json!({"permissions": {"redaction": {}}}));
        assert_eq!(level_for_call(&cfg, false, None), RedactionLevel::Basic);
        assert_eq!(level_for_call(&cfg, true, None), RedactionLevel::Credential);
    }

    #[test]
    fn an_explicit_level_promotes_every_call() {
        let cfg =
            config(serde_json::json!({"permissions": {"redaction": {"level": "credential"}}}));
        assert_eq!(
            level_for_call(&cfg, false, None),
            RedactionLevel::Credential
        );
    }

    #[test]
    fn basic_only_paths_caps_the_structural_rules() {
        let cfg = config(serde_json::json!({
            "permissions": {"redaction": {"basic_only_paths": ["**/fixtures/**"]}}
        }));
        assert_eq!(
            level_for_call(&cfg, true, Some("tests/fixtures/.env")),
            RedactionLevel::Basic,
            "a fixture keeps the high-confidence rules and is spared the rest"
        );
        assert_eq!(
            level_for_call(&cfg, true, Some(".env")),
            RedactionLevel::Credential
        );
    }

    #[test]
    fn disabled_redaction_is_off() {
        let cfg = config(serde_json::json!({"permissions": {"redaction": {"enabled": false}}}));
        assert_eq!(level_for_call(&cfg, true, None), RedactionLevel::Off);
    }

    // ── StreamRedactor ──────────────────────────────────────────────────

    #[test]
    fn stream_redactor_holds_back_a_partial_line() {
        let mut r = StreamRedactor::new(RedactionLevel::Basic);
        assert_eq!(
            r.push("harmless\nsk-7325c323"),
            Some("harmless\n".to_string())
        );
        assert_eq!(
            r.push("1cef402d8481c32a49c4898a\n"),
            Some("[redacted:api-key]\n".to_string())
        );
        assert_eq!(r.finish(), None);
    }

    /// The boundary case the hold-back exists for: the token is split across
    /// chunks and must never be emitted whole.
    #[test]
    fn stream_redactor_never_emits_a_split_secret() {
        let mut r = StreamRedactor::new(RedactionLevel::Basic);
        let mut shown = String::new();
        for chunk in ["sk-7325c32", "31cef402d84", "81c32a49c4898a\n"] {
            if let Some(out) = r.push(chunk) {
                shown.push_str(&out);
            }
        }
        if let Some(out) = r.finish() {
            shown.push_str(&out);
        }
        assert_eq!(shown, "[redacted:api-key]\n", "the token must not appear");
    }

    #[test]
    fn stream_redactor_flushes_the_holdback_at_the_end() {
        let mut r = StreamRedactor::new(RedactionLevel::Basic);
        assert_eq!(r.push("no trailing newline"), None);
        assert_eq!(r.finish(), Some("no trailing newline".to_string()));
    }

    #[test]
    fn stream_redactor_suppresses_a_private_key_block() {
        let mut r = StreamRedactor::new(RedactionLevel::Basic);
        let mut shown = String::new();
        for chunk in [
            "reading key\n-----BEGIN OPENSSH PRIVATE KEY-----\n",
            "b3BlbnNzaC1rZXktdjEAAAAABG5vbmU\n",
            "bW9yZSBzZWNyZXQ=\n",
            "-----END OPENSSH PRIVATE KEY-----\ndone\n",
        ] {
            if let Some(out) = r.push(chunk) {
                shown.push_str(&out);
            }
        }
        if let Some(out) = r.finish() {
            shown.push_str(&out);
        }
        assert!(shown.contains("reading key"), "{shown}");
        assert!(shown.contains("[redacted:private-key]"), "{shown}");
        assert!(shown.contains("done"), "{shown}");
        assert!(!shown.contains("b3BlbnNzaC1rZXk"), "{shown}");
    }

    /// A chunk that ends mid-marker must not be shown before the rest arrives.
    #[test]
    fn stream_redactor_holds_a_truncated_marker() {
        let mut r = StreamRedactor::new(RedactionLevel::Basic);
        let first = r.push("x\n-----BEGIN OPENSSH PRIV");
        assert_eq!(first, Some("x\n".to_string()), "the partial header is held");
        let rest = r.push("ATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n");
        assert!(
            !rest.unwrap_or_default().contains("AAAA"),
            "the key body must not be emitted"
        );
    }

    #[test]
    fn stream_redactor_off_passes_everything_through() {
        let mut r = StreamRedactor::new(RedactionLevel::Off);
        assert_eq!(
            r.push("sk-7325c3231cef402d8481c32a49c4898a"),
            Some("sk-7325c3231cef402d8481c32a49c4898a".to_string())
        );
        assert_eq!(r.finish(), None);
    }

    #[test]
    fn stream_redactor_flushes_a_very_long_line_instead_of_stalling() {
        let mut r = StreamRedactor::new(RedactionLevel::Basic);
        let long = "x".repeat(MAX_HOLDBACK + 10);
        let out = r.push(&long).expect("a line past the cap is emitted");
        assert_eq!(out.len(), long.len());
    }
}
