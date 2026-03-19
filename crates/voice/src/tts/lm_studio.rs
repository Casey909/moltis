//! LM Studio TTS provider implementation.
//!
//! LM Studio exposes an OpenAI-compatible `{endpoint}/audio/speech` endpoint
//! locally (default base: `http://localhost:1234/v1`). No API key is required —
//! the provider is always considered configured when the endpoint is reachable.

use {
    anyhow::{Context, Result, anyhow},
    async_trait::async_trait,
    reqwest::Client,
    serde::Serialize,
};

use super::{AudioFormat, AudioOutput, SynthesizeRequest, TtsProvider, Voice};

/// Provider identifier.
pub const STR: &str = "lm-studio";

/// Default LM Studio endpoint.
const DEFAULT_ENDPOINT: &str = "http://localhost:1234/v1";

/// Default voice (LM Studio may not support voice selection).
const DEFAULT_VOICE: &str = "default";

/// Fallback model when none is specified — LM Studio uses whichever model is loaded.
const DEFAULT_MODEL: &str = "default";

/// LM Studio TTS provider.
#[derive(Clone)]
pub struct LmStudioTts {
    client: Client,
    endpoint: String,
    default_voice: String,
    default_model: Option<String>,
}

impl std::fmt::Debug for LmStudioTts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LmStudioTts")
            .field("endpoint", &self.endpoint)
            .field("default_voice", &self.default_voice)
            .field("default_model", &self.default_model)
            .finish()
    }
}

impl Default for LmStudioTts {
    fn default() -> Self {
        Self::new(None)
    }
}

impl LmStudioTts {
    /// Create a new LM Studio TTS provider with an optional custom endpoint.
    #[must_use]
    pub fn new(endpoint: Option<String>) -> Self {
        Self {
            client: Client::new(),
            endpoint: endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.into()),
            default_voice: DEFAULT_VOICE.into(),
            default_model: None,
        }
    }

    /// Create with custom default voice, model, and endpoint.
    #[must_use]
    pub fn with_defaults(
        endpoint: Option<String>,
        voice: Option<String>,
        model: Option<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            endpoint: endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.into()),
            default_voice: voice.unwrap_or_else(|| DEFAULT_VOICE.into()),
            default_model: model,
        }
    }

    /// Map audio format to the OpenAI-compatible response format string.
    fn response_format(format: AudioFormat) -> &'static str {
        match format {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Opus | AudioFormat::Webm => "opus",
            AudioFormat::Aac => "aac",
            AudioFormat::Pcm => "pcm",
        }
    }
}

#[async_trait]
impl TtsProvider for LmStudioTts {
    fn id(&self) -> &'static str {
        STR
    }

    fn name(&self) -> &'static str {
        "LM Studio"
    }

    fn is_configured(&self) -> bool {
        true
    }

    async fn voices(&self) -> Result<Vec<Voice>> {
        Ok(vec![Voice {
            id: DEFAULT_VOICE.to_string(),
            name: "Default".to_string(),
            description: Some("Default LM Studio voice".to_string()),
            preview_url: None,
        }])
    }

    async fn synthesize(&self, request: SynthesizeRequest) -> Result<AudioOutput> {
        let voice = request
            .voice_id
            .as_deref()
            .unwrap_or(&self.default_voice);

        let model = request
            .model
            .as_deref()
            .or(self.default_model.as_deref())
            .unwrap_or(DEFAULT_MODEL);

        let body = TtsRequest {
            model,
            input: &request.text,
            voice,
            response_format: Some(Self::response_format(request.output_format)),
            speed: request.speed,
        };

        let response = self
            .client
            .post(format!("{}/audio/speech", self.endpoint))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .context("failed to send LM Studio TTS request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "LM Studio TTS request failed: {} - {}",
                status,
                body
            ));
        }

        let data = response
            .bytes()
            .await
            .context("failed to read LM Studio TTS response")?;

        Ok(AudioOutput {
            data,
            format: request.output_format,
            duration_ms: None,
        })
    }
}

// ── API Types ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct TtsRequest<'a> {
    model: &'a str,
    input: &'a str,
    voice: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<f32>,
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_metadata() {
        let provider = LmStudioTts::new(None);
        assert_eq!(provider.id(), "lm-studio");
        assert_eq!(provider.name(), "LM Studio");
        assert!(provider.is_configured());
        assert!(!provider.supports_ssml());
    }

    #[test]
    fn test_always_configured() {
        let default = LmStudioTts::default();
        assert!(default.is_configured());

        let custom = LmStudioTts::new(Some("http://custom:5000/v1".into()));
        assert!(custom.is_configured());
    }

    #[test]
    fn test_default_endpoint() {
        let provider = LmStudioTts::new(None);
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
    }

    #[test]
    fn test_custom_endpoint() {
        let provider = LmStudioTts::new(Some("http://myhost:9999/v1".into()));
        assert_eq!(provider.endpoint, "http://myhost:9999/v1");
    }

    #[test]
    fn test_response_format() {
        assert_eq!(LmStudioTts::response_format(AudioFormat::Mp3), "mp3");
        assert_eq!(LmStudioTts::response_format(AudioFormat::Opus), "opus");
        assert_eq!(LmStudioTts::response_format(AudioFormat::Webm), "opus");
        assert_eq!(LmStudioTts::response_format(AudioFormat::Aac), "aac");
        assert_eq!(LmStudioTts::response_format(AudioFormat::Pcm), "pcm");
    }

    #[test]
    fn test_debug_output() {
        let provider = LmStudioTts::new(None);
        let debug = format!("{:?}", provider);
        assert!(debug.contains("LmStudioTts"));
        assert!(debug.contains("localhost:1234"));
        assert!(debug.contains("default_voice"));
    }

    #[tokio::test]
    async fn test_voices_returns_default() {
        let provider = LmStudioTts::new(None);
        let voices = provider.voices().await.unwrap();

        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].id, "default");
        assert_eq!(voices[0].name, "Default");
        assert!(voices[0].description.is_some());
    }

    #[test]
    fn test_with_defaults() {
        let provider = LmStudioTts::with_defaults(
            Some("http://custom:8080/v1".into()),
            Some("custom-voice".into()),
            Some("my-model".into()),
        );
        assert_eq!(provider.endpoint, "http://custom:8080/v1");
        assert_eq!(provider.default_voice, "custom-voice");
        assert_eq!(provider.default_model.as_deref(), Some("my-model"));
    }

    #[test]
    fn test_with_defaults_uses_defaults() {
        let provider = LmStudioTts::with_defaults(None, None, None);
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(provider.default_voice, DEFAULT_VOICE);
        assert!(provider.default_model.is_none());
    }

    #[test]
    fn test_tts_request_serialization() {
        let request = TtsRequest {
            model: "kokoro",
            input: "Hello world",
            voice: "default",
            response_format: Some("mp3"),
            speed: Some(1.0),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"kokoro\""));
        assert!(json.contains("\"input\":\"Hello world\""));
        assert!(json.contains("\"voice\":\"default\""));
        assert!(json.contains("\"response_format\":\"mp3\""));
        assert!(json.contains("\"speed\":1.0"));
    }

    #[test]
    fn test_tts_request_serialization_skips_none() {
        let request = TtsRequest {
            model: "kokoro",
            input: "Hello",
            voice: "default",
            response_format: None,
            speed: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(!json.contains("response_format"));
        assert!(!json.contains("speed"));
    }

    #[test]
    fn test_str_constant() {
        assert_eq!(STR, "lm-studio");
    }
}
