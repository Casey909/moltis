//! LM Studio STT provider implementation.
//!
//! LM Studio exposes an OpenAI-compatible `{endpoint}/audio/transcriptions`
//! endpoint locally (default base: `http://localhost:1234/v1`). No API key is
//! required — the provider is always considered configured when the endpoint is
//! reachable.

use {
    anyhow::{Context, Result, anyhow},
    async_trait::async_trait,
    reqwest::{
        Client,
        multipart::{Form, Part},
    },
    serde::Deserialize,
};

use {
    super::{SttProvider, TranscribeRequest, Transcript, Word},
    crate::tts::AudioFormat,
};

/// Provider identifier.
pub const STR: &str = "lm-studio-stt";

/// Default LM Studio endpoint.
const DEFAULT_ENDPOINT: &str = "http://localhost:1234/v1";

/// LM Studio STT provider.
#[derive(Clone)]
pub struct LmStudioStt {
    client: Client,
    endpoint: String,
    model: Option<String>,
}

impl std::fmt::Debug for LmStudioStt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LmStudioStt")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .finish()
    }
}

impl Default for LmStudioStt {
    fn default() -> Self {
        Self::new(None)
    }
}

impl LmStudioStt {
    /// Create a new LM Studio STT provider with an optional custom endpoint.
    #[must_use]
    pub fn new(endpoint: Option<String>) -> Self {
        Self {
            client: Client::new(),
            endpoint: endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.into()),
            model: None,
        }
    }

    /// Create with custom endpoint and model.
    #[must_use]
    pub fn with_model(endpoint: Option<String>, model: Option<String>) -> Self {
        Self {
            client: Client::new(),
            endpoint: endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.into()),
            model,
        }
    }

    /// Get file extension for audio format.
    fn file_extension(format: AudioFormat) -> &'static str {
        format.extension()
    }

    /// Get MIME type for audio format.
    fn mime_type(format: AudioFormat) -> &'static str {
        format.mime_type()
    }
}

#[async_trait]
impl SttProvider for LmStudioStt {
    fn id(&self) -> &'static str {
        STR
    }

    fn name(&self) -> &'static str {
        "LM Studio"
    }

    fn is_configured(&self) -> bool {
        true
    }

    async fn transcribe(&self, request: TranscribeRequest) -> Result<Transcript> {
        let filename = format!("audio.{}", Self::file_extension(request.format));
        let mime_type = Self::mime_type(request.format);

        // Build multipart form
        let file_part = Part::bytes(request.audio.to_vec())
            .file_name(filename)
            .mime_str(mime_type)
            .context("failed to create file part")?;

        let mut form = Form::new()
            .part("file", file_part)
            .text("response_format", "verbose_json");

        if let Some(ref model) = self.model {
            form = form.text("model", model.clone());
        }

        if let Some(language) = request.language {
            form = form.text("language", language);
        }

        if let Some(prompt) = request.prompt {
            form = form.text("prompt", prompt);
        }

        let response = self
            .client
            .post(format!("{}/audio/transcriptions", self.endpoint))
            .multipart(form)
            .send()
            .await
            .context("failed to send LM Studio transcription request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "LM Studio transcription request failed: {} - {}",
                status,
                body
            ));
        }

        let lm_response: LmStudioResponse = response
            .json()
            .await
            .context("failed to parse LM Studio transcription response")?;

        Ok(Transcript {
            text: lm_response.text,
            language: lm_response.language,
            confidence: None,
            duration_seconds: lm_response.duration,
            words: lm_response.words.map(|words| {
                words
                    .into_iter()
                    .map(|w| Word {
                        word: w.word,
                        start: w.start,
                        end: w.end,
                    })
                    .collect()
            }),
        })
    }
}

// ── API Types ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct LmStudioResponse {
    text: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    duration: Option<f32>,
    #[serde(default)]
    words: Option<Vec<LmStudioWord>>,
}

#[derive(Debug, Deserialize)]
struct LmStudioWord {
    word: String,
    start: f32,
    end: f32,
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use {super::*, bytes::Bytes};

    #[test]
    fn test_provider_metadata() {
        let provider = LmStudioStt::new(None);
        assert_eq!(provider.id(), "lm-studio-stt");
        assert_eq!(provider.name(), "LM Studio");
        assert!(provider.is_configured());
    }

    #[test]
    fn test_always_configured() {
        let default = LmStudioStt::default();
        assert!(default.is_configured());

        let custom = LmStudioStt::new(Some("http://custom:5000/v1".into()));
        assert!(custom.is_configured());

        let with_model =
            LmStudioStt::with_model(Some("http://remote:8080/v1".into()), Some("whisper".into()));
        assert!(with_model.is_configured());
    }

    #[test]
    fn test_default_endpoint() {
        let provider = LmStudioStt::new(None);
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
        assert!(provider.model.is_none());
    }

    #[test]
    fn test_custom_endpoint() {
        let provider = LmStudioStt::new(Some("http://myhost:9999/v1".into()));
        assert_eq!(provider.endpoint, "http://myhost:9999/v1");
    }

    #[test]
    fn test_with_model() {
        let provider = LmStudioStt::with_model(
            Some("http://custom:8080/v1".into()),
            Some("whisper-large-v3".into()),
        );
        assert_eq!(provider.endpoint, "http://custom:8080/v1");
        assert_eq!(provider.model.as_deref(), Some("whisper-large-v3"));
    }

    #[test]
    fn test_with_model_defaults() {
        let provider = LmStudioStt::with_model(None, None);
        assert_eq!(provider.endpoint, DEFAULT_ENDPOINT);
        assert!(provider.model.is_none());
    }

    #[test]
    fn test_debug_output() {
        let provider = LmStudioStt::new(None);
        let debug = format!("{:?}", provider);
        assert!(debug.contains("LmStudioStt"));
        assert!(debug.contains("localhost:1234"));
        assert!(debug.contains("model"));
    }

    #[test]
    fn test_file_extension() {
        assert_eq!(LmStudioStt::file_extension(AudioFormat::Mp3), "mp3");
        assert_eq!(LmStudioStt::file_extension(AudioFormat::Opus), "ogg");
    }

    #[test]
    fn test_mime_type() {
        assert_eq!(LmStudioStt::mime_type(AudioFormat::Mp3), "audio/mpeg");
        assert_eq!(LmStudioStt::mime_type(AudioFormat::Opus), "audio/ogg");
    }

    #[test]
    fn test_str_constant() {
        assert_eq!(STR, "lm-studio-stt");
    }

    #[test]
    fn test_lm_studio_response_parsing() {
        let json = r#"{
            "text": "Hello, how are you?",
            "language": "en",
            "duration": 2.5,
            "words": [
                {"word": "Hello", "start": 0.0, "end": 0.5},
                {"word": "how", "start": 0.6, "end": 0.8},
                {"word": "are", "start": 0.9, "end": 1.0},
                {"word": "you", "start": 1.1, "end": 1.3}
            ]
        }"#;

        let response: LmStudioResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Hello, how are you?");
        assert_eq!(response.language, Some("en".into()));
        assert_eq!(response.duration, Some(2.5));
        assert_eq!(response.words.as_ref().unwrap().len(), 4);

        let first_word = &response.words.as_ref().unwrap()[0];
        assert_eq!(first_word.word, "Hello");
        assert!((first_word.start - 0.0).abs() < f32::EPSILON);
        assert!((first_word.end - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_lm_studio_response_minimal() {
        let json = r#"{"text": "Hello"}"#;
        let response: LmStudioResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Hello");
        assert!(response.language.is_none());
        assert!(response.duration.is_none());
        assert!(response.words.is_none());
    }

    #[test]
    fn test_lm_studio_response_without_words() {
        let json = r#"{
            "text": "Some transcribed text",
            "language": "fr",
            "duration": 5.2
        }"#;

        let response: LmStudioResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.text, "Some transcribed text");
        assert_eq!(response.language, Some("fr".into()));
        assert_eq!(response.duration, Some(5.2));
        assert!(response.words.is_none());
    }

    #[tokio::test]
    async fn test_transcribe_request_to_unreachable_host() {
        let provider = LmStudioStt::new(Some("http://192.0.2.1:1/v1".into()));
        let request = TranscribeRequest {
            audio: Bytes::from_static(b"fake audio"),
            format: AudioFormat::Mp3,
            language: None,
            prompt: None,
        };

        let result = provider.transcribe(request).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("failed to send LM Studio transcription request")
        );
    }

    #[tokio::test]
    async fn test_transcribe_with_language_and_prompt() {
        // Verify the provider builds a request with language and prompt without
        // panicking. Reaching an unreachable host is expected.
        let provider = LmStudioStt::new(Some("http://192.0.2.1:1/v1".into()));
        let request = TranscribeRequest {
            audio: Bytes::from_static(b"fake audio"),
            format: AudioFormat::Opus,
            language: Some("en".into()),
            prompt: Some("Technical discussion about Rust".into()),
        };

        let result = provider.transcribe(request).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_transcript_conversion_from_response() {
        let response = LmStudioResponse {
            text: "Test transcription".into(),
            language: Some("en".into()),
            duration: Some(3.0),
            words: Some(vec![
                LmStudioWord {
                    word: "Test".into(),
                    start: 0.0,
                    end: 0.5,
                },
                LmStudioWord {
                    word: "transcription".into(),
                    start: 0.6,
                    end: 1.5,
                },
            ]),
        };

        let transcript = Transcript {
            text: response.text,
            language: response.language,
            confidence: None,
            duration_seconds: response.duration,
            words: response.words.map(|words| {
                words
                    .into_iter()
                    .map(|w| Word {
                        word: w.word,
                        start: w.start,
                        end: w.end,
                    })
                    .collect()
            }),
        };

        assert_eq!(transcript.text, "Test transcription");
        assert_eq!(transcript.language, Some("en".into()));
        assert!(transcript.confidence.is_none());
        assert_eq!(transcript.duration_seconds, Some(3.0));
        assert_eq!(transcript.words.as_ref().unwrap().len(), 2);
        assert_eq!(transcript.words.as_ref().unwrap()[0].word, "Test");
    }
}
