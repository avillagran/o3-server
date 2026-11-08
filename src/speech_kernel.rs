//! Source-pinned speech preparation. These kernels do not activate an audio service.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

pub const MAX_SPEECH_BYTES: usize = 16_000 * 4 * 5 * 60;
const AUDIO_LENGTH: &str = "audio must contain at most five minutes of 16 kHz mono Float32 PCM";
const AUDIO_SAMPLE: &str = "audio contains an invalid PCM sample";

#[derive(Debug, PartialEq)]
pub struct InvalidAudio {
    pub byte_length: usize,
    pub message: &'static str,
}
impl std::fmt::Display for InvalidAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for InvalidAudio {}

/// Owned, bounded PCM. Only the received view is copied, never pooled backing bytes.
pub struct Pcm(Vec<f32>);
impl Pcm {
    pub fn decode(bytes: &[u8]) -> Result<Self, InvalidAudio> {
        let fail = |message| InvalidAudio {
            byte_length: bytes.len(),
            message,
        };
        if bytes.is_empty() || bytes.len() > MAX_SPEECH_BYTES || !bytes.len().is_multiple_of(4) {
            return Err(fail(AUDIO_LENGTH));
        }
        let mut samples = Vec::with_capacity(bytes.len() / 4);
        for chunk in bytes.as_chunks::<4>().0 {
            let sample = f32::from_le_bytes(*chunk);
            if !sample.is_finite() || !(-1.0..=1.0).contains(&sample) {
                return Err(fail(AUDIO_SAMPLE));
            }
            samples.push(sample);
        }
        Ok(Self(samples))
    }
    pub fn samples(&self) -> &[f32] {
        &self.0
    }
    pub fn wave(&self) -> Vec<u8> {
        let size = 44 + self.0.len() * 2;
        let mut bytes = vec![0; size];
        bytes[0..4].copy_from_slice(b"RIFF");
        bytes[4..8].copy_from_slice(&((size - 8) as u32).to_le_bytes());
        bytes[8..16].copy_from_slice(b"WAVEfmt ");
        bytes[16..20].copy_from_slice(&16u32.to_le_bytes());
        bytes[20..22].copy_from_slice(&1u16.to_le_bytes());
        bytes[22..24].copy_from_slice(&1u16.to_le_bytes());
        bytes[24..28].copy_from_slice(&16_000u32.to_le_bytes());
        bytes[28..32].copy_from_slice(&32_000u32.to_le_bytes());
        bytes[32..34].copy_from_slice(&2u16.to_le_bytes());
        bytes[34..36].copy_from_slice(&16u16.to_le_bytes());
        bytes[36..40].copy_from_slice(b"data");
        bytes[40..44].copy_from_slice(&((size - 44) as u32).to_le_bytes());
        for (index, sample) in self.0.iter().copied().enumerate() {
            let scaled = f64::from(sample) * if sample < 0.0 { 32768.0 } else { 32767.0 };
            // JavaScript Math.round rounds a negative half toward positive infinity.
            let value = (scaled + 0.5).floor() as i16;
            bytes[44 + index * 2..46 + index * 2].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
}

/// Loaded GGUF language spellings take precedence over catalog normalization.
pub fn native_language(advertised: &[String], language: &str) -> Option<String> {
    if language == "auto" {
        return None;
    }
    let tag = |value: &str| value.to_lowercase().replace('_', "-");
    let wanted = tag(language);
    Some(
        advertised
            .iter()
            .find(|entry| entry.as_str() == language)
            .or_else(|| advertised.iter().find(|entry| tag(entry) == wanted))
            .or_else(|| {
                (!wanted.contains('-'))
                    .then(|| {
                        advertised
                            .iter()
                            .find(|entry| tag(entry).split('-').next() == Some(wanted.as_str()))
                    })
                    .flatten()
            })
            .cloned()
            .unwrap_or_else(|| language.to_owned()),
    )
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelLanguages {
    pub languages: Vec<String>,
    pub supports_language_detection: bool,
}
pub fn effective_language(model: &ModelLanguages, intent: &str) -> String {
    if model.languages.len() == 1 {
        return model.languages[0].clone();
    }
    if intent != "auto" && model.languages.iter().any(|language| language == intent) {
        return intent.to_owned();
    }
    if model.supports_language_detection {
        return "auto".into();
    }
    model
        .languages
        .iter()
        .find(|language| language.as_str() == "en")
        .or_else(|| model.languages.first())
        .cloned()
        .unwrap_or_else(|| "auto".into())
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Debug)]
pub struct Word {
    pub term: String,
    pub aliases: Vec<String>,
}
#[derive(Deserialize)]
pub struct Vocabulary {
    pub personal: Vec<Word>,
    pub project: Vec<Word>,
    #[serde(default)]
    pub hints: Vec<String>,
}
fn whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
fn vocabulary_key(value: &str) -> String {
    value
        .trim_matches(whitespace)
        .nfc()
        .collect::<String>()
        .to_lowercase()
}
/// Validated dictionary entries retain their authored spelling and alias order.
/// Unicode folding follows the compiled Rust tables, with no global Node locale claim.
pub fn resolve_vocabulary(input: &Vocabulary) -> Vec<Word> {
    let known: HashSet<_> = input
        .project
        .iter()
        .chain(&input.personal)
        .flat_map(|entry| std::iter::once(&entry.term).chain(&entry.aliases))
        .map(|word| vocabulary_key(word))
        .collect();
    let hints = input.hints.iter().filter_map(|hint| {
        let term = hint
            .split(whitespace)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        (!term.is_empty()
            && term.encode_utf16().count() <= 50
            && !known.contains(&vocabulary_key(&term)))
        .then_some(Word {
            term,
            aliases: vec![],
        })
    });
    let mut seen = HashSet::new();
    hints
        .chain(input.project.iter().cloned())
        .chain(input.personal.iter().cloned())
        .filter_map(|entry| {
            if !seen.insert(vocabulary_key(&entry.term)) {
                return None;
            }
            Some(Word {
                term: entry.term,
                aliases: entry
                    .aliases
                    .into_iter()
                    .filter(|alias| seen.insert(vocabulary_key(alias)))
                    .collect(),
            })
        })
        .take(100)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    fn fixtures() -> Value {
        crate::test_sandbox::assert_isolated();
        serde_json::from_str(include_str!("../tests/fixtures/speech-kernel-4314.json")).unwrap()
    }
    fn bytes(bits: &Value) -> Vec<u8> {
        bits.as_array()
            .unwrap()
            .iter()
            .flat_map(|bit| (bit.as_u64().unwrap() as u32).to_le_bytes())
            .collect()
    }
    #[test]
    fn whole_native_pcm_views_and_wave_rounding() {
        let f = fixtures();
        assert_eq!(f["revision"], "83a3674ffd84ffb33de768210c806cb1ac53fd80");
        assert_eq!(f["pcms"].as_array().unwrap().len(), 20);
        for row in f["pcms"].as_array().unwrap() {
            let result = if row.get("bits").is_some() {
                let input = bytes(&row["bits"]);
                let mut backing = [255; 20];
                backing[8..12].copy_from_slice(&input);
                Pcm::decode(&backing[8..12])
            } else {
                Pcm::decode(&vec![0; row["byteLength"].as_u64().unwrap() as usize])
            };
            if row["allowed"] == true {
                let pcm = result.unwrap();
                if let Some(bits) = row.get("decodedBits") {
                    assert_eq!(
                        json!(pcm
                            .samples()
                            .iter()
                            .map(|sample| sample.to_bits())
                            .collect::<Vec<_>>()),
                        *bits
                    );
                } else {
                    assert_eq!(
                        pcm.samples().len(),
                        row["samples"].as_u64().unwrap() as usize
                    );
                }
            } else {
                let error = result.err().unwrap();
                assert_eq!(
                    json!({"tag":"SpeechInvalidAudioError","message":error.message,"byteLength":error.byte_length}),
                    row["error"]
                );
            }
        }
        assert_eq!(f["waves"].as_array().unwrap().len(), 4);
        for row in f["waves"].as_array().unwrap() {
            let samples = bytes(&row["bits"])
                .as_chunks::<4>()
                .0
                .iter()
                .map(|chunk| f32::from_le_bytes(*chunk))
                .collect();
            // The native pure WAV function also defines the empty-header boundary.
            let actual = Pcm(samples).wave();
            let encoded: String = actual.iter().map(|byte| format!("{byte:02x}")).collect();
            assert_eq!(encoded, row["waveHex"]);
        }
    }
    #[test]
    fn whole_native_language_and_vocabulary_projection() {
        let f = fixtures();
        assert_eq!(f["languages"].as_array().unwrap().len(), 66);
        for row in f["languages"].as_array().unwrap() {
            assert_eq!(
                json!(native_language(
                    &serde_json::from_value::<Vec<String>>(row["advertised"].clone()).unwrap(),
                    row["wanted"].as_str().unwrap()
                )),
                row["result"]
            );
        }
        assert_eq!(f["effective"].as_array().unwrap().len(), 20);
        for row in f["effective"].as_array().unwrap() {
            let model = serde_json::from_value(row["model"].clone()).unwrap();
            assert_eq!(
                effective_language(&model, row["intent"].as_str().unwrap()),
                row["result"]
            );
        }
        assert_eq!(f["vocabularies"].as_array().unwrap().len(), 27);
        for row in f["vocabularies"].as_array().unwrap() {
            assert_eq!(
                json!(resolve_vocabulary(
                    &serde_json::from_value(row["input"].clone()).unwrap()
                )),
                row["result"]
            );
        }
    }
}
