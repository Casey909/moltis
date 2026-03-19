//! macOS native TTS provider using the built-in `say` command.
//!
//! The `say` command is available on all macOS systems and supports a wide
//! variety of voices across many languages. No API key, model download, or
//! external server is required.
//!
//! ```text
//! say -v Alex "Hello, world"                          # speak to speakers
//! say -v Alex -o out.aiff --data-format=LEI16@22050   # write raw PCM to file
//! say -v '?'                                          # list available voices
//! ```

use {
    anyhow::{Context, Result, anyhow},
    async_trait::async_trait,
    bytes::Bytes,
    std::path::PathBuf,
    tokio::process::Command,
};

use super::{AudioFormat, AudioOutput, SynthesizeRequest, TtsProvider, Voice};

/// Provider identifier.
pub const STR: &str = "macos";

/// Default voice when none is specified.
const DEFAULT_VOICE: &str = "Alex";

/// Normal speaking rate in words per minute for the `say` command.
const DEFAULT_RATE_WPM: u32 = 175;

/// Sample rate used in the `--data-format` flag (Hz).
const SAMPLE_RATE: u32 = 22050;

/// macOS native TTS provider.
///
/// Delegates to the `/usr/bin/say` binary shipped with every macOS installation.
/// Audio is produced as 16-bit little-endian signed PCM at 22 050 Hz.
#[derive(Debug, Clone)]
pub struct MacOsTts {
    default_voice: String,
}

impl Default for MacOsTts {
    fn default() -> Self {
        Self::new(None)
    }
}

impl MacOsTts {
    /// Create a new macOS TTS provider.
    ///
    /// An optional `default_voice` overrides the fallback voice (`"Alex"`).
    #[must_use]
    pub fn new(default_voice: Option<String>) -> Self {
        Self {
            default_voice: default_voice.unwrap_or_else(|| DEFAULT_VOICE.into()),
        }
    }

    /// Build the temporary file path for `say` output.
    fn temp_output_path() -> PathBuf {
        let mut path = std::env::temp_dir();
        let id = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        path.push(format!("moltis-say-{id}-{ts}.aiff"));
        path
    }

    /// Convert a speed multiplier (typically 0.5–2.0) to `say --rate` words-per-minute.
    fn speed_to_rate(speed: Option<f32>) -> u32 {
        speed
            .map(|s| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let rate = (DEFAULT_RATE_WPM as f32 * s) as u32;
                rate.clamp(1, 700)
            })
            .unwrap_or(DEFAULT_RATE_WPM)
    }

    /// Parse a single line from `say -v '?'` output.
    ///
    /// Expected format:
    /// ```text
    /// Alex                en_US    # Most people recognize me by my voice.
    /// ```
    fn parse_voice_line(line: &str) -> Option<Voice> {
        // Name is everything before the first language token.  The language
        // token looks like `xx_XX` and is separated from the name by
        // whitespace.  After the language there is `#` followed by the
        // description.
        let hash_pos = line.find('#')?;

        let before_hash = line[..hash_pos].trim();

        // Split `before_hash` into name + language.  The language code is the
        // last whitespace-separated token (e.g. "en_US").
        let last_space = before_hash.rfind(char::is_whitespace)?;
        let name = before_hash[..last_space].trim();
        if name.is_empty() {
            return None;
        }

        let description = line[hash_pos + 1..].trim();
        let desc = if description.is_empty() {
            None
        } else {
            Some(description.to_string())
        };

        Some(Voice {
            id: name.to_string(),
            name: name.to_string(),
            description: desc,
            preview_url: None,
        })
    }
}

#[async_trait]
impl TtsProvider for MacOsTts {
    fn id(&self) -> &'static str {
        STR
    }

    fn name(&self) -> &'static str {
        "macOS"
    }

    fn is_configured(&self) -> bool {
        cfg!(target_os = "macos")
    }

    async fn voices(&self) -> Result<Vec<Voice>> {
        let output = Command::new("say")
            .arg("-v")
            .arg("?")
            .output()
            .await
            .context("failed to run `say -v '?'` — is this a macOS system?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("`say -v '?'` failed: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let voices: Vec<Voice> = stdout
            .lines()
            .filter_map(Self::parse_voice_line)
            .collect();

        Ok(voices)
    }

    async fn synthesize(&self, request: SynthesizeRequest) -> Result<AudioOutput> {
        let voice = request
            .voice_id
            .as_deref()
            .unwrap_or(&self.default_voice);

        let rate = Self::speed_to_rate(request.speed);

        let out_path = Self::temp_output_path();

        let mut cmd = Command::new("say");
        cmd.arg("-v").arg(voice);
        cmd.arg("--rate").arg(rate.to_string());
        cmd.arg("-o").arg(&out_path);
        cmd.arg(format!("--data-format=LEI16@{SAMPLE_RATE}"));
        cmd.arg(&request.text);

        let output = cmd
            .output()
            .await
            .context("failed to spawn `say` — is this a macOS system?")?;

        if !output.status.success() {
            // Clean up temp file on failure (best effort).
            let _ = tokio::fs::remove_file(&out_path).await;
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("`say` synthesis failed: {}", stderr));
        }

        let data = tokio::fs::read(&out_path)
            .await
            .context("failed to read `say` output file")?;

        // Always clean up the temp file.
        let _ = tokio::fs::remove_file(&out_path).await;

        // The `--data-format=LEI16@22050` flag produces an AIFF file with
        // 16-bit little-endian signed PCM payload.  The AIFF container headers
        // are included in the returned data — callers that need raw PCM samples
        // must strip the headers or transcode (e.g. via ffmpeg).
        let (data, format) = match request.output_format {
            AudioFormat::Pcm => (Bytes::from(data), AudioFormat::Pcm),
            AudioFormat::Mp3 | AudioFormat::Opus | AudioFormat::Aac | AudioFormat::Webm => {
                // Return as PCM; the caller / gateway can transcode if needed.
                (Bytes::from(data), AudioFormat::Pcm)
            },
        };

        Ok(AudioOutput {
            data,
            format,
            duration_ms: None,
        })
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    // ── Construction ───────────────────────────────────────────────────

    #[test]
    fn test_default_voice() {
        let tts = MacOsTts::new(None);
        assert_eq!(tts.default_voice, DEFAULT_VOICE);
    }

    #[test]
    fn test_custom_voice() {
        let tts = MacOsTts::new(Some("Samantha".into()));
        assert_eq!(tts.default_voice, "Samantha");
    }

    #[test]
    fn test_default_trait() {
        let tts = MacOsTts::default();
        assert_eq!(tts.default_voice, DEFAULT_VOICE);
    }

    // ── Provider metadata ──────────────────────────────────────────────

    #[test]
    fn test_id() {
        let tts = MacOsTts::new(None);
        assert_eq!(tts.id(), "macos");
    }

    #[test]
    fn test_name() {
        let tts = MacOsTts::new(None);
        assert_eq!(tts.name(), "macOS");
    }

    #[test]
    fn test_str_constant() {
        assert_eq!(STR, "macos");
    }

    #[test]
    fn test_does_not_support_ssml() {
        let tts = MacOsTts::new(None);
        assert!(!tts.supports_ssml());
    }

    #[test]
    fn test_is_configured() {
        let tts = MacOsTts::new(None);
        // On non-macOS CI this returns false; on macOS it returns true.
        assert_eq!(tts.is_configured(), cfg!(target_os = "macos"));
    }

    // ── Speed → rate conversion ────────────────────────────────────────

    #[test]
    fn test_speed_none_returns_default_rate() {
        assert_eq!(MacOsTts::speed_to_rate(None), DEFAULT_RATE_WPM);
    }

    #[test]
    fn test_speed_1x_returns_default_rate() {
        assert_eq!(MacOsTts::speed_to_rate(Some(1.0)), DEFAULT_RATE_WPM);
    }

    #[test]
    fn test_speed_2x_doubles_rate() {
        assert_eq!(MacOsTts::speed_to_rate(Some(2.0)), DEFAULT_RATE_WPM * 2);
    }

    #[test]
    fn test_speed_half_halves_rate() {
        assert_eq!(MacOsTts::speed_to_rate(Some(0.5)), DEFAULT_RATE_WPM / 2);
    }

    #[test]
    fn test_speed_clamps_low() {
        assert_eq!(MacOsTts::speed_to_rate(Some(0.001)), 1);
    }

    #[test]
    fn test_speed_clamps_high() {
        assert_eq!(MacOsTts::speed_to_rate(Some(100.0)), 700);
    }

    // ── Voice line parsing ─────────────────────────────────────────────

    #[test]
    fn test_parse_standard_line() {
        let line = "Alex                en_US    # Most people recognize me by my voice.";
        let voice = MacOsTts::parse_voice_line(line).unwrap();
        assert_eq!(voice.id, "Alex");
        assert_eq!(voice.name, "Alex");
        assert_eq!(
            voice.description.as_deref(),
            Some("Most people recognize me by my voice.")
        );
        assert!(voice.preview_url.is_none());
    }

    #[test]
    fn test_parse_multi_word_name() {
        let line = "Bad News            en_US    # The strains of the news grow tedious.";
        let voice = MacOsTts::parse_voice_line(line).unwrap();
        assert_eq!(voice.id, "Bad News");
        assert_eq!(voice.name, "Bad News");
        assert_eq!(
            voice.description.as_deref(),
            Some("The strains of the news grow tedious.")
        );
    }

    #[test]
    fn test_parse_non_english_locale() {
        let line = "Thomas              fr_FR    # Bonjour, je m'appelle Thomas.";
        let voice = MacOsTts::parse_voice_line(line).unwrap();
        assert_eq!(voice.id, "Thomas");
        assert_eq!(voice.name, "Thomas");
        assert_eq!(
            voice.description.as_deref(),
            Some("Bonjour, je m'appelle Thomas.")
        );
    }

    #[test]
    fn test_parse_no_description() {
        let line = "Alex                en_US    #";
        let voice = MacOsTts::parse_voice_line(line).unwrap();
        assert_eq!(voice.name, "Alex");
        assert!(voice.description.is_none());
    }

    #[test]
    fn test_parse_empty_line_returns_none() {
        assert!(MacOsTts::parse_voice_line("").is_none());
    }

    #[test]
    fn test_parse_no_hash_returns_none() {
        assert!(MacOsTts::parse_voice_line("Alex en_US no hash").is_none());
    }

    #[test]
    fn test_parse_only_whitespace_name_returns_none() {
        let line = "    en_US    # description";
        assert!(MacOsTts::parse_voice_line(line).is_none());
    }

    // ── Temp path generation ───────────────────────────────────────────

    #[test]
    fn test_temp_path_is_in_temp_dir() {
        let path = MacOsTts::temp_output_path();
        assert!(path.starts_with(std::env::temp_dir()));
    }

    #[test]
    fn test_temp_path_has_aiff_extension() {
        let path = MacOsTts::temp_output_path();
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("aiff"));
    }

    #[test]
    fn test_temp_paths_are_unique() {
        let a = MacOsTts::temp_output_path();
        // Small sleep to ensure different nanosecond timestamp.
        std::thread::sleep(std::time::Duration::from_nanos(1));
        let b = MacOsTts::temp_output_path();
        assert_ne!(a, b);
    }

    // ── Debug output ───────────────────────────────────────────────────

    #[test]
    fn test_debug() {
        let tts = MacOsTts::new(None);
        let debug = format!("{tts:?}");
        assert!(debug.contains("MacOsTts"));
        assert!(debug.contains("Alex"));
    }

    // ── Synthesis (unit-level, no macOS required) ──────────────────────

    #[tokio::test]
    async fn test_synthesize_fails_gracefully_on_non_macos() {
        if cfg!(target_os = "macos") {
            // Skip on actual macOS — the command would succeed.
            return;
        }
        let tts = MacOsTts::new(None);
        let req = SynthesizeRequest {
            text: "Hello".into(),
            ..Default::default()
        };
        let result = tts.synthesize(req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_voices_fails_gracefully_on_non_macos() {
        if cfg!(target_os = "macos") {
            return;
        }
        let tts = MacOsTts::new(None);
        let result = tts.voices().await;
        assert!(result.is_err());
    }
}
