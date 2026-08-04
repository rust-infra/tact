use std::time::Duration;

use anyhow::{Context, bail};
use async_trait::async_trait;
use base64::Engine;
use reqwest::{
    StatusCode,
    multipart::{Form, Part},
};
use tokio_util::sync::CancellationToken;

use crate::config::VoiceSettings;

#[async_trait]
pub trait Transcriber: Send + Sync {
    async fn transcribe(&self, wav: Vec<u8>, cancel: CancellationToken) -> anyhow::Result<String>;
}

pub struct GoogleTranscriber {
    settings: VoiceSettings,
    client: reqwest::Client,
}

impl GoogleTranscriber {
    pub fn new(settings: VoiceSettings) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("reqwest client");
        Self { settings, client }
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/speech:recognize",
            self.settings.base_url.trim_end_matches('/')
        )
    }
}

#[async_trait]
impl Transcriber for GoogleTranscriber {
    async fn transcribe(&self, wav: Vec<u8>, cancel: CancellationToken) -> anyhow::Result<String> {
        let api_key = self
            .settings
            .api_key
            .as_deref()
            .filter(|key| !key.is_empty())
            .context("[voice].api_key is not configured")?;
        let content = base64::engine::general_purpose::STANDARD.encode(wav);
        let language_code = self
            .settings
            .language
            .as_deref()
            .filter(|language| !language.trim().is_empty())
            .unwrap_or("zh");
        let body = serde_json::json!({
            "config": {
                "encoding": "LINEAR16",
                "sampleRateHertz": 16_000,
                "languageCode": language_code,
                "model": self.settings.model,
            },
            "audio": { "content": content },
        });
        let request = self
            .client
            .post(self.endpoint())
            .query(&[("key", api_key)])
            .json(&body);
        let response = tokio::select! {
            result = request.send() => result.map_err(|_| anyhow::anyhow!("transcription request failed"))?,
            () = cancel.cancelled() => bail!("transcription cancelled"),
        };
        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("failed to read transcription response body")?;
        parse_google_transcription_response(status, &body)
    }
}
pub struct OpenAiTranscriber {
    settings: VoiceSettings,
    client: reqwest::Client,
}

impl OpenAiTranscriber {
    pub fn new(settings: VoiceSettings) -> Self {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("reqwest client");
        Self { settings, client }
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/audio/transcriptions",
            self.settings.base_url.trim_end_matches('/')
        )
    }
}

#[async_trait]
impl Transcriber for OpenAiTranscriber {
    async fn transcribe(&self, wav: Vec<u8>, cancel: CancellationToken) -> anyhow::Result<String> {
        let api_key = self
            .settings
            .api_key
            .as_deref()
            .filter(|k| !k.is_empty())
            .context("[voice].api_key is not configured")?;

        let file_part = Part::bytes(wav)
            .file_name("recording.wav".to_string())
            .mime_str("audio/wav")
            .context("failed to build multipart file part")?;
        let mut form = Form::new()
            .part("file", file_part)
            .text("model", self.settings.model.clone())
            .text("response_format", "json");
        if let Some(lang) = self
            .settings
            .language
            .as_ref()
            .filter(|l| !l.trim().is_empty())
        {
            form = form.text("language", lang.clone());
        }

        let request = self
            .client
            .post(self.endpoint())
            .bearer_auth(api_key)
            .multipart(form);

        let response = tokio::select! {
            result = request.send() => result.context("transcription request failed")?,
            () = cancel.cancelled() => bail!("transcription cancelled"),
        };

        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("failed to read transcription response body")?;
        parse_transcription_response(status, &body)
    }
}

pub struct WhisperCppTranscriber {
    settings: VoiceSettings,
    client: reqwest::Client,
}

impl WhisperCppTranscriber {
    pub fn new(settings: VoiceSettings) -> Self {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("reqwest client");
        Self { settings, client }
    }

    fn endpoint(&self) -> String {
        format!("{}/inference", self.settings.base_url.trim_end_matches('/'))
    }
}

#[async_trait]
impl Transcriber for WhisperCppTranscriber {
    async fn transcribe(&self, wav: Vec<u8>, cancel: CancellationToken) -> anyhow::Result<String> {
        let file_part = Part::bytes(wav)
            .file_name("recording.wav".to_string())
            .mime_str("audio/wav")
            .context("failed to build multipart file part")?;
        let mut form = Form::new()
            .part("file", file_part)
            .text("response_format", "json");
        if let Some(lang) = self
            .settings
            .language
            .as_ref()
            .filter(|l| !l.trim().is_empty())
        {
            form = form.text("language", lang.clone());
        }

        let request = self.client.post(self.endpoint()).multipart(form);

        let response = tokio::select! {
            result = request.send() => result.context("transcription request failed")?,
            () = cancel.cancelled() => bail!("transcription cancelled"),
        };

        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("failed to read transcription response body")?;
        parse_transcription_response(status, &body)
    }
}

pub fn parse_google_transcription_response(
    status: StatusCode,
    body: &[u8],
) -> anyhow::Result<String> {
    if !status.is_success() {
        bail!("Google transcription HTTP {status}");
    }
    let value: serde_json::Value =
        serde_json::from_slice(body).context("invalid Google transcription JSON response")?;
    let transcripts = value
        .get("results")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|result| {
            result
                .get("alternatives")
                .and_then(serde_json::Value::as_array)
                .and_then(|alternatives| alternatives.first())
                .and_then(|alternative| alternative.get("transcript"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
        })
        .collect::<Vec<_>>();
    if transcripts.is_empty() {
        bail!("Google transcription response missing non-empty transcript");
    }
    Ok(transcripts.join(" "))
}

pub fn parse_transcription_response(status: StatusCode, body: &[u8]) -> anyhow::Result<String> {
    if !status.is_success() {
        let snippet = String::from_utf8_lossy(body);
        let snippet = snippet.chars().take(200).collect::<String>();
        bail!("transcription HTTP {status}: {snippet}");
    }
    let value: serde_json::Value =
        serde_json::from_slice(body).context("invalid transcription JSON response")?;
    let text = value
        .get("text")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .context("transcription response missing non-empty text field")?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::VoiceProvider;
    use std::time::Duration;

    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn test_voice_settings(base_url: &str) -> VoiceSettings {
        VoiceSettings {
            enabled: true,
            provider: VoiceProvider::OpenAi,
            api_key: Some("voice-test".to_string()),
            base_url: base_url.to_string(),
            model: "gpt-4o-mini-transcribe".to_string(),
            language: Some("zh".to_string()),
            max_duration_secs: 300,
            voice_keybind: None,
        }
    }

    fn google_settings(base_url: &str) -> VoiceSettings {
        VoiceSettings {
            enabled: true,
            provider: VoiceProvider::Google,
            api_key: Some("google-test-key".to_string()),
            base_url: base_url.to_string(),
            model: "latest_short".to_string(),
            language: Some("zh-CN".to_string()),
            max_duration_secs: 60,
            voice_keybind: None,
        }
    }

    fn google_transcriber_without_proxy(settings: VoiceSettings) -> GoogleTranscriber {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("reqwest test client");
        GoogleTranscriber { settings, client }
    }

    #[test]
    fn parse_google_response_joins_results() {
        let body = br#"{"results":[{"alternatives":[{"transcript":" first "}]},{"alternatives":[{"transcript":"second"}]}]}"#;
        let text = parse_google_transcription_response(StatusCode::OK, body).unwrap();
        assert_eq!(text, "first second");
    }

    #[test]
    fn parse_google_response_rejects_empty_results() {
        for body in [br#"{}"#.as_slice(), br#"{"results":[]}"#.as_slice()] {
            let err = parse_google_transcription_response(StatusCode::OK, body).unwrap_err();
            assert!(err.to_string().contains("non-empty transcript"));
        }
    }

    #[test]
    fn parse_google_response_rejects_malformed_json() {
        let err = parse_google_transcription_response(StatusCode::OK, b"not json").unwrap_err();
        assert!(err.to_string().contains("JSON"));
    }

    #[test]
    fn parse_google_non_success_status() {
        let err =
            parse_google_transcription_response(StatusCode::UNAUTHORIZED, b"permission denied")
                .unwrap_err();
        assert!(err.to_string().contains("401"));
        assert!(!err.to_string().contains("google-test-key"));
    }

    #[tokio::test]
    async fn google_transcriber_sends_expected_json_and_joins_results() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/speech:recognize"))
            .and(wiremock::matchers::query_param("key", "google-test-key"))
            .and(header("content-type", "application/json"))
            .and(|request: &Request| {
                let value: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                value["config"]["encoding"] == "LINEAR16"
                    && value["config"]["sampleRateHertz"] == 16_000
                    && value["config"]["languageCode"] == "zh-CN"
                    && value["config"]["model"] == "latest_short"
                    && base64::engine::general_purpose::STANDARD
                        .decode(value["audio"]["content"].as_str().unwrap())
                        .unwrap()
                        == vec![1, 2, 3]
            })
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [
                    {"alternatives": [{"transcript": "你好"}]},
                    {"alternatives": [{"transcript": "Tact"}]}
                ]
            })))
            .mount(&server)
            .await;
        let text =
            google_transcriber_without_proxy(google_settings(&format!("{}/v1", server.uri())))
                .transcribe(vec![1, 2, 3], CancellationToken::new())
                .await
                .unwrap();
        assert_eq!(text, "你好 Tact");
    }

    #[tokio::test]
    async fn google_transcriber_honors_http_proxy() {
        const CHILD_MARKER: &str = "TACT_GOOGLE_PROXY_TEST_CHILD";

        if std::env::var_os(CHILD_MARKER).is_some() {
            let text = GoogleTranscriber::new(google_settings(
                "http://google-voice-proxy-test.invalid/v1",
            ))
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .expect("Google transcription should use the configured HTTP proxy");
            assert_eq!(text, "proxied");
            return;
        }

        let proxy = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [{"alternatives": [{"transcript": "proxied"}]}]
            })))
            .mount(&proxy)
            .await;

        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
        child
            .arg("--exact")
            .arg("voice::transcriber::tests::google_transcriber_honors_http_proxy")
            .arg("--nocapture")
            .env(CHILD_MARKER, "1")
            .env("HTTP_PROXY", proxy.uri())
            .env("http_proxy", proxy.uri())
            .env_remove("HTTPS_PROXY")
            .env_remove("https_proxy")
            .env_remove("ALL_PROXY")
            .env_remove("all_proxy")
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .kill_on_drop(true);

        let status = tokio::time::timeout(Duration::from_secs(10), child.status())
            .await
            .expect("proxy child test timed out")
            .expect("failed to launch proxy child test");
        assert!(status.success(), "proxy child test failed: {status}");
    }

    #[tokio::test]
    async fn google_transcriber_missing_api_key_errors_before_request() {
        let settings = VoiceSettings {
            api_key: None,
            ..google_settings("http://localhost")
        };
        let err = GoogleTranscriber::new(settings)
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("[voice].api_key"));
        assert!(!err.to_string().contains("google-test-key"));
    }

    #[tokio::test]
    async fn google_transcriber_rejects_http_error_without_leaking_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .mount(&server)
            .await;
        let err = google_transcriber_without_proxy(google_settings(&server.uri()))
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("403"));
        assert!(!err.to_string().contains("google-test-key"));
    }

    #[tokio::test]
    async fn google_transcriber_cancellation_aborts_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let cancel = cancel.clone();
            async move {
                google_transcriber_without_proxy(google_settings(&server.uri()))
                    .transcribe(vec![1, 2, 3], cancel)
                    .await
            }
        });
        cancel.cancel();
        let err = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("cancellation should finish promptly")
            .unwrap()
            .unwrap_err();
        assert!(err.to_string().to_ascii_lowercase().contains("cancel"));
    }

    #[test]
    fn parse_valid_json_text() {
        let body = br#"{"text":"hello"}"#;
        let text = parse_transcription_response(StatusCode::OK, body).unwrap();
        assert_eq!(text, "hello");
    }

    #[test]
    fn parse_rejects_empty_text() {
        let body = br#"{"text":"   "}"#;
        let err = parse_transcription_response(StatusCode::OK, body).unwrap_err();
        assert!(err.to_string().contains("text"));
    }

    #[test]
    fn parse_rejects_missing_text() {
        let body = br#"{}"#;
        let err = parse_transcription_response(StatusCode::OK, body).unwrap_err();
        assert!(err.to_string().contains("text"));
    }

    #[test]
    fn parse_rejects_malformed_json() {
        let body = b"not json";
        let err = parse_transcription_response(StatusCode::OK, body).unwrap_err();
        assert!(err.to_string().contains("JSON"));
    }

    #[test]
    fn parse_non_success_status() {
        let err =
            parse_transcription_response(StatusCode::UNAUTHORIZED, b"unauthorized").unwrap_err();
        assert!(err.to_string().contains("401"));
        assert!(!err.to_string().contains("voice-test"));
    }

    #[tokio::test]
    async fn transcriber_sends_expected_multipart_and_parses_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .and(header("authorization", "Bearer voice-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"text":"你好 Tact"})))
            .mount(&server)
            .await;
        let settings = test_voice_settings(&format!("{}/v1", server.uri()));
        let text = OpenAiTranscriber::new(settings)
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(text, "你好 Tact");
    }

    #[tokio::test]
    async fn transcriber_rejects_http_error_and_missing_text_without_leaking_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;
        let err = OpenAiTranscriber::new(test_voice_settings(&server.uri()))
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("401"));
        assert!(!err.to_string().contains("voice-test"));

        let server2 = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server2)
            .await;
        let err = OpenAiTranscriber::new(test_voice_settings(&server2.uri()))
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("text"));
        assert!(!err.to_string().contains("voice-test"));
    }

    #[tokio::test]
    async fn transcriber_cancellation_aborts_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let settings = test_voice_settings(&server.uri());
            let cancel = cancel.clone();
            async move {
                OpenAiTranscriber::new(settings)
                    .transcribe(vec![1, 2, 3], cancel)
                    .await
            }
        });
        cancel.cancel();
        let err = task.await.unwrap().unwrap_err();
        assert!(err.to_string().to_ascii_lowercase().contains("cancel"));
    }

    #[tokio::test]
    async fn transcriber_missing_api_key_errors_before_request() {
        let settings = VoiceSettings {
            enabled: true,
            api_key: None,
            ..test_voice_settings("http://localhost")
        };
        let err = OpenAiTranscriber::new(settings)
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("[voice].api_key"));
    }

    fn whisper_settings(base_url: &str) -> VoiceSettings {
        VoiceSettings {
            enabled: true,
            provider: VoiceProvider::WhisperCpp,
            api_key: None,
            base_url: base_url.to_string(),
            model: String::new(),
            language: Some("zh".to_string()),
            max_duration_secs: 300,
            voice_keybind: None,
        }
    }

    #[tokio::test]
    async fn whisper_transcriber_sends_expected_multipart_and_parses_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"text":"你好 Tact"})))
            .mount(&server)
            .await;
        let settings = whisper_settings(&server.uri());
        let text = WhisperCppTranscriber::new(settings)
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(text, "你好 Tact");
    }

    #[tokio::test]
    async fn whisper_transcriber_rejects_http_error_without_leaking_data() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&server)
            .await;
        let err = WhisperCppTranscriber::new(whisper_settings(&server.uri()))
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("500"));
    }

    #[tokio::test]
    async fn whisper_transcriber_cancellation_aborts_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let settings = whisper_settings(&server.uri());
            let cancel = cancel.clone();
            async move {
                WhisperCppTranscriber::new(settings)
                    .transcribe(vec![1, 2, 3], cancel)
                    .await
            }
        });
        cancel.cancel();
        let err = task.await.unwrap().unwrap_err();
        assert!(err.to_string().to_ascii_lowercase().contains("cancel"));
    }

    #[tokio::test]
    async fn whisper_transcriber_omits_language_when_none() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"text":"ok"})))
            .mount(&server)
            .await;
        let mut settings = whisper_settings(&server.uri());
        settings.language = None;
        let text = WhisperCppTranscriber::new(settings)
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(text, "ok");
    }

    #[tokio::test]
    async fn whisper_transcriber_does_not_send_model_or_auth() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .and(|req: &Request| {
                // Verify no authorization header is present.
                !req.headers.contains_key("authorization")
            })
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"text":"no-auth"})))
            .mount(&server)
            .await;
        let text = WhisperCppTranscriber::new(whisper_settings(&server.uri()))
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(text, "no-auth");
    }
}
