//! Error recovery and retry logic.
//!
//! The agent loop uses this module to decide whether a failure is transient
//! (network timeout, rate limit) or permanent (prompt too long).  Transient
//! errors are retried with exponential back-off (see [`backoff_delay`]).
//!
//! - [`CONTINUATION_MESSAGE`]: appended when the LLM hits its output limit
//!   on the first attempt, asking it to pick up mid-response.
//! - [`CONVERGENCE_CONTINUATION_MESSAGE`]: used on repeated truncations to
//!   stop further expansion and request only a concise final result.
//! - [`continuation_message`]: selector that chooses the direct-resume prompt
//!   on attempt 1 and the convergence prompt on later attempts.
//! - [`MAX_COMPACT_ATTEMPTS`]: prompt-too-long compaction retries.
//! - [`MAX_TRANSPORT_ATTEMPTS`]: transient network error retries (higher for
//!   long-running tasks that may encounter multiple intermittent failures).
//! - [`MAX_CONTINUATION_ATTEMPTS`]: max-tokens continuation retries.
//! - [`MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS`]: transient retries during the
//!   compaction summary call itself.
//! - [`MAX_COMPACT_SUMMARY_ATTEMPTS`]: compaction-summarizer attempts before a
//!   partial summary is accepted as best-effort.
//! - [`RecoveryState`]: tracks attempts across compaction, continuation, and
//!   transport categories.
//! - [`is_prompt_too_long_error`] / [`is_transient_transport_error`]:
//!   classify error strings to route recovery decisions.
//! - [`error_summary`]: collapses an error into a single readable line so
//!   recovery status messages say *why* a retry is happening.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MAX_COMPACT_ATTEMPTS: u32 = 3;
pub const MAX_TRANSPORT_ATTEMPTS: u32 = 10;
pub const MAX_CONTINUATION_ATTEMPTS: u32 = 3;
/// Retries for **transient transport** errors during the compaction summary
/// LLM call (timeouts, 408/429/5xx); a permanent error is never retried.
/// Kept below [`MAX_TRANSPORT_ATTEMPTS`] because the summary call is a short
/// one-shot operation, but raised to give a flaky link more chances before
/// compaction is abandoned and surfaced to the main loop.
///
/// This budget does **not** affect truncation-driven fallback; that is
/// governed by [`MAX_COMPACT_SUMMARY_ATTEMPTS`].
pub const MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS: u32 = 5;
/// Compaction-summarizer attempts before a partial summary is accepted as
/// best-effort. Kept separate from [`MAX_CONTINUATION_ATTEMPTS`], which the
/// main agent loop also uses: compaction wants more patience because an
/// output-limited summary is still a usable handoff, while the main loop
/// should not keep re-answering an over-long response indefinitely.
pub const MAX_COMPACT_SUMMARY_ATTEMPTS: u32 = 5;
const BACKOFF_BASE_DELAY_SECS: f64 = 1.0;
const BACKOFF_MAX_DELAY_SECS: f64 = 30.0;

pub const CONTINUATION_MESSAGE: &str = "Output limit hit. Continue directly from where you stopped. \
No recap, no repetition. Pick up mid-sentence if needed.";

pub const CONVERGENCE_CONTINUATION_MESSAGE: &str = "Your response has been truncated repeatedly. Stop expanding the analysis and do not revisit the same scenarios. Return only the final actionable result in a concise structured format: conclusion, verified issues, and minimal fixes. Do not recap, repeat, or speculate.";

/// Returns the continuation prompt appropriate for the current attempt index.
///
/// During continuation recovery, attempt 1 uses the direct-resume prompt. If the model
/// is truncated again, attempts 2 and 3 switch to a convergence prompt that requests
/// only concise actionable conclusions without further expansion.
pub fn continuation_message(attempt: u32) -> &'static str {
    if attempt <= 1 {
        CONTINUATION_MESSAGE
    } else {
        CONVERGENCE_CONTINUATION_MESSAGE
    }
}

/// Current state of retry counters.
///
/// Each counter is scoped to a recovery strategy:
/// - `continuation_attempts`: "output limit" continuations.
/// - `compact_attempts`: context-compaction attempts.
/// - `transport_attempts`: network-level retries.
#[derive(Debug, Default)]
pub struct RecoveryState {
    pub continuation_attempts: u32,
    pub compact_attempts: u32,
    pub transport_attempts: u32,
}

/// Returns `true` if the error string indicates the prompt exceeded the
/// model's context window.
pub fn is_prompt_too_long_error(error_text: &str) -> bool {
    (error_text.contains("prompt") && error_text.contains("long"))
        || error_text.contains("overlong_prompt")
        || error_text.contains("too many tokens")
        || error_text.contains("context length")
}

/// Returns `true` if the error string matches a known transient transport
/// failure pattern (timeout, rate limit, connection reset, etc.).
///
/// Prefer [`is_transient_llm_error`] when the typed error is still available:
/// string matching cannot tell a retryable 503 from a permanent 400, and it
/// depends on the provider's English prose.
pub fn is_transient_transport_error(error_text: &str) -> bool {
    [
        "timeout",
        "timed out",
        "rate limit",
        "too many requests",
        "unavailable",
        "connection",
        "overloaded",
        "temporarily",
        "econnreset",
        "broken pipe",
        "http request failed",
        "error sending request",
    ]
    .iter()
    .any(|needle| error_text.contains(needle))
}

/// Whether an HTTP status code describes a failure worth retrying.
///
/// `429` / `408` (rate limit, request timeout) and every `5xx` are transient;
/// other `4xx` (400 malformed request, 401/403 credential or authorization,
/// 404) are permanent — retrying them only burns the user's quota.
pub fn is_transient_http_status(status: u16) -> bool {
    status == 429 || status == 408 || (500..600).contains(&status)
}

/// Classification of a failed LLM call for the recovery loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Prompt exceeded the model's context window; compact and retry.
    PromptTooLong,
    /// Retryable transport/rate-limit failure; back off and retry.
    Transient,
    /// Everything else — surface to the caller.
    Permanent,
}

/// Message-based fallback classification, used for errors whose type carries
/// no status (transport failures arrive as `reqwest` prose).
fn classify_text(error_text: &str) -> FailureKind {
    let lowered = error_text.to_lowercase();
    if is_prompt_too_long_error(&lowered) {
        FailureKind::PromptTooLong
    } else if is_transient_transport_error(&lowered) {
        FailureKind::Transient
    } else {
        FailureKind::Permanent
    }
}

/// Classifies an [`LlmError`](tact_llm::LlmError), preferring its typed fields.
///
/// `HttpError` carries the real HTTP status, so a 429/5xx is retried while a
/// 400/401/403/404 fails fast instead of burning the user's quota. `Request`
/// means the transport never finished — a dropped connection, a read timeout,
/// or a stream cut before its terminal chunk — and is transient by definition,
/// which is how [`tact_llm::neutral::model_error`] classifies the same variant.
/// Classifying it by prose instead made a truncated stream permanent whenever
/// its wording happened to miss the keyword list. The remaining variants carry
/// neither a status nor a transport meaning, so they fall back to the text.
pub fn classify_llm_error(error: &tact_llm::LlmError) -> FailureKind {
    if let tact_llm::LlmError::HttpError { class, .. } = error {
        return match class {
            tact_llm::HttpErrorClass::ContextOverflow => FailureKind::PromptTooLong,
            tact_llm::HttpErrorClass::Transient => FailureKind::Transient,
            tact_llm::HttpErrorClass::Authentication | tact_llm::HttpErrorClass::Permanent => {
                FailureKind::Permanent
            }
        };
    }
    if matches!(error, tact_llm::LlmError::Request(_)) {
        return FailureKind::Transient;
    }
    classify_text(&error.to_string())
}

/// Classifies the provider-neutral adapter error without converting it back
/// into a provider-specific `LlmError` just to recover its retry class.
pub fn classify_model_error(error: &tact_agent_api::ModelError) -> FailureKind {
    match error.kind {
        tact_agent_api::ModelErrorKind::ContextOverflow => FailureKind::PromptTooLong,
        tact_agent_api::ModelErrorKind::Transient => FailureKind::Transient,
        // `Authentication` is not retryable: the same key produces the same 401
        // no matter how many times it is sent. Everything unknown is treated
        // the same way — retrying something we cannot classify only spends the
        // user's quota.
        _ => FailureKind::Permanent,
    }
}

/// What a user should see when a model call failed.
///
/// The adapter layer cannot produce this: it knows the class of failure but not
/// which provider, model, or endpoint is configured, and it deliberately keeps
/// provider text out of its message. The product layer knows all of that, so
/// this is where a sentence with an action in it can be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFailureAdvice {
    /// One line: what happened, naming provider/model/host where known.
    pub headline: String,
    /// One line: what to change. `None` only for cancellation, which is not a
    /// failure the user has to fix.
    pub action: Option<String>,
    /// Whether resending unchanged could succeed.
    pub retryable: bool,
}

/// Builds the user-facing text for a failed model call.
///
/// `metadata` is `None` when the call never reached a built provider (an
/// unavailable snapshot), in which case the text stays generic instead of
/// inventing a provider name.
pub fn model_failure_advice(
    error: &tact_agent_api::ModelError,
    metadata: Option<&tact_llm::ProviderMetadata>,
) -> ModelFailureAdvice {
    use tact_agent_api::ModelErrorKind;

    let provider = metadata
        .map(|metadata| metadata.provider().to_string())
        .unwrap_or_else(|| "provider".to_string());
    let model = metadata
        .map(|metadata| metadata.model().to_string())
        .unwrap_or_else(|| "the configured model".to_string());
    let host = metadata
        .map(|metadata| metadata.endpoint_host())
        .unwrap_or_default();
    let at = if host.is_empty() {
        String::new()
    } else {
        format!(" at {host}")
    };

    match error.kind {
        ModelErrorKind::Cancelled => ModelFailureAdvice {
            headline: "model call cancelled".to_string(),
            action: None,
            retryable: false,
        },
        ModelErrorKind::ContextOverflow => ModelFailureAdvice {
            headline: format!(
                "the conversation no longer fits {model}'s context window ({provider})"
            ),
            action: Some(
                "run /compact to summarize the session, raise the context window, or start a new \
                 session"
                    .to_string(),
            ),
            retryable: false,
        },
        ModelErrorKind::Transient => ModelFailureAdvice {
            headline: format!(
                "{provider} is temporarily unreachable{at}, and every automatic retry failed"
            ),
            action: Some(
                "check the network or proxy, then resend the message; nothing in the request \
                 needs to change"
                    .to_string(),
            ),
            retryable: true,
        },
        ModelErrorKind::Unsupported => ModelFailureAdvice {
            headline: format!("{provider} does not support this operation"),
            action: Some(
                "switch provider or turn off the feature that requested it; the request itself \
                 cannot be made compatible"
                    .to_string(),
            ),
            retryable: false,
        },
        ModelErrorKind::Authentication => ModelFailureAdvice {
            headline: format!("{provider} rejected the credential for {model}{at}"),
            action: Some(credentials_action(&provider, metadata.is_some())),
            retryable: false,
        },
        // `Permanent` and any kind a newer adapter adds: the adapter's message
        // is already body-free and often names the concrete mismatch (a
        // rejected status, a model that does not match, a cursor from another
        // endpoint). Keeping it is the difference between "something failed"
        // and "this is what to fix".
        _ => {
            let headline = match host.as_str() {
                "" => format!("{provider} rejected the request for {model}"),
                _ => format!("{provider} rejected the request for {model} at {host}"),
            };
            let action = permanent_action(error, metadata);
            ModelFailureAdvice {
                headline,
                action: Some(action),
                retryable: false,
            }
        }
    }
}

/// Where this provider's key comes from, as a sentence.
///
/// Naming the settings path is the whole point: "authentication failed" without
/// a location makes the user search the docs, and the location is already known
/// here. The environment variable is `TACT_API_KEY` — the one the CLI actually
/// reads (`config/cli.rs`) — not a per-provider `FOO_API_KEY`, which exists only
/// in this repo's live-test conventions and would send the user somewhere that
/// does nothing.
fn credentials_action(provider: &str, has_metadata: bool) -> String {
    if has_metadata {
        format!(
            "set a working API key for {provider} in [llm.providers.{provider}] api_key \
             (config.toml or .tact/settings.json), or pass --api-key / set TACT_API_KEY, then \
             resend the message"
        )
    } else {
        format!(
            "set a working API key for {provider} in [llm.providers.{provider}] api_key, then \
             resend the message"
        )
    }
}

/// The actionable half of a permanent failure.
///
/// Reads the adapter's message for the one thing it cannot say: which setting
/// it was. The message text is Tact's own (see `LlmError::user_message`), never
/// a provider body, so matching on it is not reading provider prose.
fn permanent_action(
    error: &tact_agent_api::ModelError,
    metadata: Option<&tact_llm::ProviderMetadata>,
) -> String {
    let message = error.message.to_ascii_lowercase();
    let provider = metadata
        .map(|metadata| metadata.provider().to_string())
        .unwrap_or_else(|| "this provider".to_string());

    // A 401/403 can still arrive as `Permanent` from an adapter that reports
    // the status inside a protocol-level error (the Anthropic path wraps its
    // HTTP failures in `MessageError::ApiError`). Same fix, same sentence.
    if message.contains("authentication")
        || message.contains("credential")
        || message.contains("http 401")
        || message.contains("http 403")
    {
        return credentials_action(&provider, metadata.is_some());
    }
    if message.contains("max_output_tokens") || message.contains("max_tokens") {
        return "lower max_tokens (Settings → Max tokens) or raise limits.max_output_tokens for \
                this provider"
            .to_string();
    }
    if message.contains("does not match the provider's configured model") {
        return "switch to the model this provider was built for (/model), or rebuild the provider \
                by re-selecting it"
            .to_string();
    }
    if message.contains("conversation state") || message.contains("cursor") {
        return "start a new session; the saved conversation state belongs to a different provider \
                or endpoint"
            .to_string();
    }
    if message.contains("not valid json") || message.contains("stream was malformed") {
        return "check base_url for this provider: the endpoint did not return this protocol's \
                JSON, which usually means a wrong path or a proxy error page"
            .to_string();
    }
    if message.contains("safety classifier") || message.contains("refusal") {
        return "rephrase the request, or switch to a model with different safety filters; \
                resending it unchanged will be refused again"
            .to_string();
    }
    if message.contains("http 404") {
        return "check the model name and base_url for this provider; the endpoint or model does \
                not exist"
            .to_string();
    }
    if message.contains("http 400") || message.contains("http 422") {
        return "the provider rejected the request shape; check the model name and any per-model \
                options for this provider"
            .to_string();
    }
    format!(
        "check the model name, base_url, and credentials for {provider}; the provider rejected \
         the request"
    )
}

/// Classifies a boxed error, downcasting to [`tact_llm::LlmError`] when the
/// typed cause survived (see `Agent::stream_message`).
pub fn classify_error(error: &anyhow::Error) -> FailureKind {
    if let Some(model_error) = model_error_of(error) {
        return classify_model_error(model_error);
    }
    match error.downcast_ref::<tact_llm::LlmError>() {
        Some(llm_error) => classify_llm_error(llm_error),
        None => classify_text(&error.to_string()),
    }
}

/// The typed model error inside a boxed failure, if one survived.
///
/// The kernel now wraps the whole [`tact_agent_api::ModelError`] instead of a
/// string (see `KernelError::Model`), so the failure class is still recoverable
/// at the host boundary. Hosts use this to build user-facing text without
/// taking a direct dependency on the agent API crate.
pub fn model_error_of(error: &anyhow::Error) -> Option<&tact_agent_api::ModelError> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<tact_agent_api::ModelError>())
}

/// Exponential back-off delay with millisecond jitter.
///
/// Formula: `min(1s × 2^attempt, 30s) + random(0..1s)`.
pub fn backoff_delay(attempt: u32) -> Duration {
    let base = (BACKOFF_BASE_DELAY_SECS * 2f64.powi(attempt as i32)).min(BACKOFF_MAX_DELAY_SECS);
    let jitter = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| (duration.subsec_millis() % 1000) as f64 / 1000.0)
        .unwrap_or(0.0);
    Duration::from_secs_f64(base + jitter)
}

/// Maximum length of the single-line error summary embedded in recovery
/// status messages. Long chains are truncated so the TUI line stays readable.
const MAX_ERROR_SUMMARY_CHARS: usize = 200;

/// Collapses an error message into a single readable line for recovery
/// status messages: runs of whitespace (including newlines) become single
/// spaces, and overly long messages are truncated with an ellipsis.
pub fn error_summary(error_text: &str) -> String {
    let collapsed: String = error_text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_ERROR_SUMMARY_CHARS {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(MAX_ERROR_SUMMARY_CHARS).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http_error(status: u16, body: &str) -> tact_llm::LlmError {
        let class = if body.contains("context length") || body.contains("prompt is too long") {
            tact_llm::HttpErrorClass::ContextOverflow
        } else if status == 401 || status == 403 {
            tact_llm::HttpErrorClass::Authentication
        } else if is_transient_http_status(status) {
            tact_llm::HttpErrorClass::Transient
        } else {
            tact_llm::HttpErrorClass::Permanent
        };
        tact_llm::LlmError::HttpError {
            status,
            class,
            retry_after: None,
        }
    }

    #[test]
    fn http_status_classification_retries_only_transient_codes() {
        for status in [408, 429, 500, 502, 503, 504] {
            assert_eq!(
                classify_llm_error(&http_error(status, "busy")),
                FailureKind::Transient,
                "{status} should be retryable"
            );
        }
    }

    /// The whole point of the typed classification: a permanent client error
    /// must not be retried just because its body happens to read like a
    /// transient one.
    #[test]
    fn permanent_client_errors_are_not_retried() {
        for status in [400, 401, 403, 404, 422] {
            assert_eq!(
                classify_llm_error(&http_error(status, "rate limit exceeded")),
                FailureKind::Permanent,
                "{status} must not be retried"
            );
        }
    }

    /// An over-long prompt is usually a 400, and the recovery loop fixes it by
    /// compacting rather than retrying verbatim.
    #[test]
    fn overlong_prompt_on_a_400_is_recoverable() {
        assert_eq!(
            classify_llm_error(&http_error(400, "context length exceeded")),
            FailureKind::PromptTooLong
        );
    }

    #[test]
    fn transport_textual_errors_still_classify() {
        assert_eq!(
            classify_llm_error(&tact_llm::LlmError::Request("connection reset".into())),
            FailureKind::Transient
        );
        assert_eq!(
            classify_llm_error(&tact_llm::LlmError::Auth("bad key".into())),
            FailureKind::Permanent
        );
    }

    /// A cut SSE stream has to be retried, not surfaced as a permanent failure.
    ///
    /// The chat-completions adapter reports truncation as `Request`, which is
    /// the variant that means "the transport did not finish". This pins the
    /// classification the fix depends on: if the adapter ever reports it as
    /// `StreamParse` instead, the turn fails permanently and the retry that
    /// recovers the work never happens.
    #[test]
    fn a_truncated_stream_is_worth_retrying() {
        assert_eq!(
            classify_llm_error(&tact_llm::LlmError::Request(
                "SSE stream ended without `[DONE]` or a finish_reason after 11 bytes of text"
                    .into()
            )),
            FailureKind::Transient
        );
    }

    #[test]
    fn neutral_model_error_kinds_survive_classification() {
        use tact_agent_api::{ModelError, ModelErrorKind};

        let overflow = anyhow::Error::from(ModelError::new(
            ModelErrorKind::ContextOverflow,
            "neutral overflow",
        ));
        let transient = anyhow::Error::from(ModelError::new(
            ModelErrorKind::Transient,
            "neutral transport",
        ));
        let permanent = anyhow::Error::from(ModelError::new(
            ModelErrorKind::Permanent,
            "rate limit wording must not override the type",
        ));

        assert_eq!(classify_error(&overflow), FailureKind::PromptTooLong);
        assert_eq!(classify_error(&transient), FailureKind::Transient);
        assert_eq!(classify_error(&permanent), FailureKind::Permanent);
    }

    /// `Agent::stream_message` preserves the typed error, so the downcast path
    /// is what the recovery loop actually exercises.
    #[test]
    fn boxed_errors_are_classified_through_the_typed_cause() {
        let boxed = anyhow::Error::from(http_error(429, "slow down"));
        assert_eq!(classify_error(&boxed), FailureKind::Transient);

        let stringified = anyhow::anyhow!("api error (429): slow down");
        assert_eq!(
            classify_error(&stringified),
            FailureKind::Permanent,
            "without the type there is nothing to distinguish a 429 from prose"
        );
    }

    #[test]
    fn is_transient_matches_http_request_failed() {
        assert!(is_transient_transport_error(
            "unsupported response state: HTTP request failed: error sending request for url"
        ));
    }

    #[test]
    fn is_transient_matches_timeout() {
        assert!(is_transient_transport_error("request timed out"));
    }

    #[test]
    fn is_transient_matches_econnreset() {
        assert!(is_transient_transport_error("econnreset"));
    }

    #[test]
    fn is_transient_rejects_prompt_too_long() {
        assert!(!is_transient_transport_error("prompt too long"));
    }

    #[test]
    fn first_continuation_preserves_direct_resume_prompt() {
        assert_eq!(continuation_message(0), CONTINUATION_MESSAGE);
        assert_eq!(continuation_message(1), CONTINUATION_MESSAGE);
    }

    #[test]
    fn repeated_continuations_switch_to_convergence_prompt() {
        assert_eq!(continuation_message(2), CONVERGENCE_CONTINUATION_MESSAGE);
        assert_eq!(continuation_message(3), CONVERGENCE_CONTINUATION_MESSAGE);
        assert_eq!(continuation_message(99), CONVERGENCE_CONTINUATION_MESSAGE);
    }

    #[test]
    fn convergence_prompt_requires_concise_actionable_output() {
        let prompt = CONVERGENCE_CONTINUATION_MESSAGE.to_ascii_lowercase();
        assert!(prompt.contains("stop"));
        assert!(prompt.contains("concise"));
        assert!(prompt.contains("actionable"));
        assert!(prompt.contains("repeat"));
    }

    #[test]
    fn error_summary_preserves_short_errors() {
        assert_eq!(error_summary("request timed out"), "request timed out");
    }

    #[test]
    fn error_summary_collapses_whitespace_and_truncates() {
        let long = format!("line one\nline two   line three{}", " x".repeat(300));
        let summary = error_summary(&long);
        assert!(!summary.contains('\n'));
        assert!(!summary.contains("  "));
        assert_eq!(summary.chars().count(), MAX_ERROR_SUMMARY_CHARS + 1);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn error_summary_keeps_chain_separators() {
        let summary = error_summary("http request failed: error sending request for url");
        assert_eq!(
            summary,
            "http request failed: error sending request for url"
        );
    }

    fn metadata(provider: &str, model: &str, endpoint: &str) -> tact_llm::ProviderMetadata {
        tact_llm::custom_chat(provider, model, endpoint)
            .anonymous()
            .limits(tact_llm::ModelLimits::new(Some(200_000), 8_192).expect("limits"))
            .build()
            .expect("a custom provider builds without credentials")
            .metadata()
            .clone()
    }

    /// The whole point of the advice: a permanent failure names something the
    /// reader can change. "provider request failed" was the regression.
    #[test]
    fn a_permanent_failure_names_a_provider_and_an_action() {
        use tact_agent_api::{ModelError, ModelErrorKind};

        let metadata = metadata("deepseek", "deepseek-v4.1", "https://api.deepseek.com");
        let error = ModelError::new(ModelErrorKind::Permanent, "provider returned HTTP 401");
        let advice = model_failure_advice(&error, Some(&metadata));

        assert!(advice.headline.contains("deepseek"), "{}", advice.headline);
        assert!(
            advice.headline.contains("deepseek-v4.1"),
            "{}",
            advice.headline
        );
        assert!(
            advice.headline.contains("api.deepseek.com"),
            "{}",
            advice.headline
        );
        let action = advice.action.expect("a permanent failure has an action");
        assert!(action.contains("API key"), "{action}");
        assert!(!advice.retryable);
    }

    #[test]
    fn a_rejected_model_name_points_at_the_model_setting() {
        use tact_agent_api::{ModelError, ModelErrorKind};

        let error = ModelError::new(
            ModelErrorKind::Permanent,
            "request model 'x' does not match the provider's configured model 'y'; \
             switch the model (/model) or rebuild the provider",
        );
        let advice = model_failure_advice(&error, None);
        assert!(advice.headline.contains("provider"), "{}", advice.headline);
        let action = advice.action.clone().expect("action");
        assert!(action.contains("/model"), "{action}");
    }

    #[test]
    fn a_transient_failure_is_the_one_that_offers_a_retry() {
        use tact_agent_api::{ModelError, ModelErrorKind};

        let metadata = metadata("kimi", "kimi-k2", "https://api.moonshot.cn");
        let error = ModelError::new(ModelErrorKind::Transient, "unreachable");
        let advice = model_failure_advice(&error, Some(&metadata));
        assert!(advice.retryable);
        assert!(advice.headline.contains("kimi"), "{}", advice.headline);
        assert!(advice.action.expect("action").contains("network"));
    }

    /// A credential problem is not a request problem: no retry button, and the
    /// action names where the key lives so the user does not have to search.
    #[test]
    fn a_rejected_credential_names_where_the_key_lives() {
        use tact_agent_api::{ModelError, ModelErrorKind};

        let metadata = metadata("anthropic", "claude-test", "https://api.anthropic.com");
        let error = ModelError::new(
            ModelErrorKind::Authentication,
            "provider returned HTTP 401 (authentication failed)",
        );
        let advice = model_failure_advice(&error, Some(&metadata));

        assert!(!advice.retryable, "the same key would fail again");
        assert!(
            advice.headline.contains("credential"),
            "{}",
            advice.headline
        );
        let action = advice.action.expect("action");
        assert!(action.contains("llm.providers.anthropic"), "{action}");
        // `TACT_API_KEY` is the variable the CLI actually reads. The
        // per-provider `FOO_API_KEY` spelling exists only in this repo's
        // ignored live tests, so pointing a user at it would be a dead end.
        assert!(action.contains("TACT_API_KEY"), "{action}");
        assert!(!action.contains("ANTHROPIC_API_KEY"), "{action}");
    }

    /// Authentication must not be classified as retryable by the recovery loop
    /// either, or the kernel would burn the budget against a dead key.
    #[test]
    fn authentication_is_never_retried() {
        use tact_agent_api::{ModelError, ModelErrorKind};

        let error = ModelError::new(ModelErrorKind::Authentication, "401");
        assert_eq!(classify_model_error(&error), FailureKind::Permanent);
        assert!(!error.is_retryable());
    }

    #[test]
    fn a_context_overflow_points_at_compaction() {
        use tact_agent_api::{ModelError, ModelErrorKind};

        let error = ModelError::new(ModelErrorKind::ContextOverflow, "too large");
        let advice = model_failure_advice(&error, None);
        assert!(advice.action.expect("action").contains("/compact"));
        assert!(!advice.retryable);
    }

    /// Cancellation is not a failure the reader has to fix, so it must not
    /// grow an "action" line telling them to change a setting.
    #[test]
    fn cancellation_carries_no_action() {
        use tact_agent_api::{ModelError, ModelErrorKind};

        let error = ModelError::new(ModelErrorKind::Cancelled, "model call cancelled");
        assert!(model_failure_advice(&error, None).action.is_none());
    }

    /// The advice is rendered to users, so it must never carry a provider body
    /// even when the adapter message is the one being inspected.
    #[test]
    fn advice_never_echoes_a_provider_body() {
        use tact_agent_api::{ModelError, ModelErrorKind};

        let error = ModelError::new(
            ModelErrorKind::Permanent,
            "provider returned HTTP 400 (request rejected)",
        );
        let advice = model_failure_advice(&error, None);
        let rendered = format!("{} {}", advice.headline, advice.action.unwrap_or_default());
        assert!(!rendered.contains("SECRET"), "{rendered}");
        // The body-free wording the adapter emits is what the advice reads.
        assert!(!rendered.contains("{\"error\""), "{rendered}");
    }
}
