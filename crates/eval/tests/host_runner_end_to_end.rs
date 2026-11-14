//! End-to-end: a real bootstrap, a real agent loop, and a real grader.
//!
//! The model is a local wiremock server speaking the OpenAI chat protocol, so
//! this test exercises the whole harness (config install, session open, skill
//! registry, kernel, event pump, grading) without a network call.
//!
//! One test, not several: the agent configuration is process-global, so a
//! second `#[tokio::test]` in this binary would race the first for it. The two
//! scenarios share one server and are told apart by what the prompt asks for.

#![allow(clippy::result_large_err)]

use std::path::PathBuf;

use tact_eval::{EvalSuite, HostRunner, RunOptions, run_suite};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// One streamed chat turn, as OpenAI sends it.
///
/// Built from JSON values rather than a raw string so the assistant text is
/// escaped. The agent streams, so a plain completion body is read as an empty
/// stream and the run retries until it fails.
fn completion_sse(text: &str) -> String {
    let chunk = serde_json::json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 1_750_000_000u64,
        "model": "eval-model",
        "choices": [{
            "index": 0,
            "delta": { "role": "assistant", "content": text },
            "finish_reason": "stop"
        }],
    });
    let usage = serde_json::json!({
        "id": "chatcmpl-test",
        "object": "chat.completion.chunk",
        "created": 1_750_000_000u64,
        "model": "eval-model",
        "choices": [],
        "usage": {
            "prompt_tokens": 11,
            "completion_tokens": 3,
            "total_tokens": 14
        },
    });
    format!("data: {chunk}\n\ndata: {usage}\n\ndata: [DONE]\n\n")
}

fn unique_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tact-eval-it-{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_case(suite_dir: &std::path::Path, name: &str, prompt: &str) {
    write_case_with_timeout(suite_dir, name, prompt, 60);
}

fn write_case_with_timeout(
    suite_dir: &std::path::Path,
    name: &str,
    prompt: &str,
    timeout_secs: u64,
) {
    std::fs::write(
        suite_dir.join(format!("{name}.toml")),
        format!(
            r#"
prompt = "{prompt}"
timeout_secs = {timeout_secs}

[[graders]]
kind = "command"
name = "ran"
command = "true"
"#
        ),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_run_reports_the_answer_stop_reason_usage_and_verdict() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains("please answer"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(completion_sse("hello from eval"), "text/event-stream"),
        )
        .mount(&server)
        .await;
    // Slower than the case's own budget, so the harness cancels it.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_string_contains("reply slowly"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(completion_sse("too late"), "text/event-stream")
                .set_delay(std::time::Duration::from_secs(30)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(completion_sse(""), "text/event-stream"),
        )
        .mount(&server)
        .await;

    // An explicit config file, not flags plus whatever `$HOME` holds.
    //
    // `load_toml_config` writes the default template into `$HOME/.tact/` on a
    // first run and then reads it back, and that template sets
    // `[llm.providers.openai] protocol = "responses"`. On a machine without a
    // config, the run would therefore call `/responses` while this test serves
    // `/chat/completions`. Naming the protocol here makes the test independent
    // of the machine it runs on.
    let config_path = unique_dir("config").join("tact.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "eval-model"
base_url = "{base}"
protocol = "chat_completions"
"#,
            base = server.uri()
        ),
    )
    .unwrap();
    let options = RunOptions {
        config: Some(config_path.clone()),
        permission_mode: tact_eval::PermissionMode::Auto,
        ..Default::default()
    };
    tact::config::init_from_args(&options.to_cli_args()).expect("config install");

    let suite_dir = unique_dir("suite");
    write_case(&suite_dir, "answers", "please answer");
    write_case(&suite_dir, "silent", "say nothing");
    write_case_with_timeout(&suite_dir, "slow", "reply slowly", 1);
    let suite = EvalSuite::load(&suite_dir).unwrap();

    let run = run_suite(&suite, &HostRunner::new(options), 1)
        .await
        .unwrap();

    let answered = run
        .outcomes
        .iter()
        .find(|outcome| outcome.case_id == "answers")
        .expect("answers case ran");
    assert_eq!(
        answered.answer, "hello from eval",
        "the final message text reaches the report (failure: {:?})",
        answered.failure
    );
    assert!(
        answered.verdict().is_pass(),
        "the grader should pass: {:?}",
        answered.grades
    );
    assert_eq!(answered.tokens.prompt, 11);
    assert_eq!(answered.tokens.completion, 3);
    assert_eq!(
        answered.stop,
        Some(tact_eval::StopView::Completed),
        "the host path reports the typed stop reason the CLI path cannot see"
    );
    assert_eq!(answered.turns, Some(1), "one model call is one turn");

    let silent = run
        .outcomes
        .iter()
        .find(|outcome| outcome.case_id == "silent")
        .expect("silent case ran");
    assert_eq!(
        silent.stop,
        Some(tact_eval::StopView::EmptyTurn),
        "an empty answer must not be reported as Completed"
    );
    assert_eq!(silent.answer, "");

    let slow = run
        .outcomes
        .iter()
        .find(|outcome| outcome.case_id == "slow")
        .expect("slow case ran");
    // The harness cancelled this run, so its passing grader must not make it a
    // pass: the model never finished.
    assert_eq!(
        slow.failure,
        Some(tact_eval::RunFailure::Timeout { limit_secs: 1 }),
        "an over-budget run is a timeout"
    );
    assert!(
        !slow.verdict().is_pass(),
        "a run the harness stopped cannot pass: {:?}",
        slow.verdict()
    );

    let _ = std::fs::remove_dir_all(&suite_dir);
}
