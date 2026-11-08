//! Native speech request schemas and bounded binary-envelope decoding.
use crate::{
    managed_settings_codec as codec,
    speech_kernel::{InvalidAudio, Pcm},
};
use serde_json::Value;

pub const MAX_OPTIONS_BYTES: usize = 128_000;
#[derive(Debug, PartialEq)]
pub enum RequestError {
    MissingOptions,
    InvalidLength,
    InvalidEncoding,
    InvalidJson,
    InvalidOptions,
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MissingOptions => "Missing speech options.",
            Self::InvalidLength => "Invalid speech options length.",
            Self::InvalidEncoding => "The encoded data was not valid for encoding utf-8",
            Self::InvalidJson => "Expected a valid JSON string",
            Self::InvalidOptions => "Invalid speech options.",
        })
    }
}
impl std::error::Error for RequestError {}

pub struct TranscriptionOptions(Value);
impl TranscriptionOptions {
    pub fn decode(value: Value) -> Result<Self, RequestError> {
        codec::speech_options(value, false, false)
            .map(Self)
            .map_err(|()| RequestError::InvalidOptions)
    }
    pub fn as_value(&self) -> &Value {
        &self.0
    }
    pub fn encoded(&self) -> Result<Value, RequestError> {
        codec::speech_options(self.0.clone(), false, true)
            .map_err(|()| RequestError::InvalidOptions)
    }
}
pub struct PostProcessingRequest(Value);
impl PostProcessingRequest {
    pub fn decode(value: Value) -> Result<Self, RequestError> {
        codec::speech_options(value, true, false)
            .map(Self)
            .map_err(|()| RequestError::InvalidOptions)
    }
    pub fn as_value(&self) -> &Value {
        &self.0
    }
    pub fn encoded(&self) -> Result<Value, RequestError> {
        codec::speech_options(self.0.clone(), true, true).map_err(|()| RequestError::InvalidOptions)
    }
}

/// This decodes the native envelope; PCM validation remains a separate owned step.
pub struct BinaryRequest<'a> {
    pub options: TranscriptionOptions,
    pub pcm: &'a [u8],
}
impl<'a> BinaryRequest<'a> {
    pub fn decode(body: &'a [u8]) -> Result<Self, RequestError> {
        if body.len() < 4 {
            return Err(RequestError::MissingOptions);
        }
        let length =
            u32::from_le_bytes(body[..4].try_into().expect("four-byte options length")) as usize;
        if length > MAX_OPTIONS_BYTES || 4 + length >= body.len() {
            return Err(RequestError::InvalidLength);
        }
        let text =
            std::str::from_utf8(&body[4..4 + length]).map_err(|_| RequestError::InvalidEncoding)?;
        // WHATWG TextDecoder strips one leading UTF-8 BOM, never embedded or repeated BOMs.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let value = serde_json::from_str(text).map_err(|_| RequestError::InvalidJson)?;
        let options = TranscriptionOptions::decode(value)?;
        Ok(Self {
            options,
            pcm: &body[4 + length..],
        })
    }
    pub fn owned_pcm(&self) -> Result<Pcm, InvalidAudio> {
        Pcm::decode(self.pcm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(name: &str) -> Value {
        crate::test_sandbox::assert_isolated();
        serde_json::from_str(match name {
            "options" => include_str!("../tests/fixtures/speech-options-4314.json"),
            "request" => include_str!("../tests/fixtures/speech-request-4314.json"),
            _ => unreachable!(),
        })
        .unwrap()
    }
    #[test]
    fn actual_closed_speech_option_schemas() {
        let f = fixture("options");
        assert_eq!(f["revision"], "83a3674ffd84ffb33de768210c806cb1ac53fd80");
        assert_eq!(f["cases"].as_array().unwrap().len(), 72);
        for row in f["cases"].as_array().unwrap() {
            let (decoded, encoded) = if row["kind"] == "transcription" {
                match TranscriptionOptions::decode(row["input"].clone()) {
                    Ok(value) => (Ok(value.as_value().clone()), value.encoded()),
                    Err(error) => (Err(error), Err(RequestError::InvalidOptions)),
                }
            } else {
                match PostProcessingRequest::decode(row["input"].clone()) {
                    Ok(value) => (Ok(value.as_value().clone()), value.encoded()),
                    Err(error) => (Err(error), Err(RequestError::InvalidOptions)),
                }
            };
            if row["invalid"] == true {
                assert!(decoded.is_err(), "{row}");
            } else {
                assert_eq!(decoded, Ok(row["decoded"].clone()), "{row}");
                assert_eq!(encoded, Ok(row["encoded"].clone()), "{row}");
            }
        }
    }
    fn hex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                let digit = |c: u8| if c <= b'9' { c - b'0' } else { c - b'a' + 10 };
                (digit(pair[0]) << 4) | digit(pair[1])
            })
            .collect()
    }
    #[test]
    fn actual_binary_parser_bom_fatal_utf8_and_pooled_views() {
        let f = fixture("request");
        assert_eq!(f["cases"].as_array().unwrap().len(), 18);
        for row in f["cases"].as_array().unwrap() {
            let raw = hex(row["bodyHex"].as_str().unwrap());
            let offset = row["offset"].as_u64().unwrap() as usize;
            let mut backing = vec![255; offset + raw.len() + 11];
            backing[offset..offset + raw.len()].copy_from_slice(&raw);
            let result = BinaryRequest::decode(&backing[offset..offset + raw.len()]);
            if row["allowed"] == true {
                let request = result.unwrap_or_else(|error| panic!("{}: {error}", row["name"]));
                assert_eq!(
                    request.options.as_value(),
                    &row["options"],
                    "{}",
                    row["name"]
                );
                assert_eq!(request.pcm, hex(row["pcmHex"].as_str().unwrap()));
                if row["name"] == "one-pcm-byte" {
                    assert!(request.owned_pcm().is_err());
                } else {
                    assert!(request.owned_pcm().is_ok());
                }
            } else {
                assert_eq!(
                    result.err().unwrap().to_string(),
                    row["error"]["message"],
                    "{}",
                    row["name"]
                );
            }
        }
    }
}
