use super::error::{Result, ScmError};
use serde::Serialize;
use serde_json::Value;
use std::cmp::Ordering;

/// Encodes the RFC 8785 JSON Canonicalization Scheme.
///
/// Rust strings cannot contain unpaired UTF-16 surrogates. Object keys are
/// ordered by their UTF-16 code units as required by JCS. Number formatting is
/// delegated to `serde_jcs`, which implements the ECMAScript representation.
pub fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let value = serde_json::to_value(value)?;
    let mut output = Vec::new();
    encode_value(&value, &mut output)?;
    Ok(output)
}

fn encode_value(value: &Value, output: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(value) => output.extend_from_slice(if *value { b"true" } else { b"false" }),
        Value::Number(number) => {
            if let Some(byte) = number.as_u64().filter(|value| *value <= 255) {
                // Every byte has this exact decimal representation in JCS.
                // Avoid allocating a serializer/output buffer for each entry
                // of byte arrays. Floats (including -0), negatives and larger
                // integers retain the general ECMAScript number formatter.
                let byte = byte as u8;
                if byte >= 100 {
                    output.push(b'0' + byte / 100);
                }
                if byte >= 10 {
                    output.push(b'0' + (byte / 10) % 10);
                }
                output.push(b'0' + byte % 10);
            } else {
                output.extend_from_slice(
                    &serde_jcs::to_vec(number)
                        .map_err(|error| ScmError::Serialization(error.to_string()))?,
                );
            }
        }
        Value::String(value) => output.extend_from_slice(serde_json::to_string(value)?.as_bytes()),
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                encode_value(value, output)?;
            }
            output.push(b']');
        }
        Value::Object(values) => {
            let mut keys: Vec<&str> = values.keys().map(String::as_str).collect();
            keys.sort_by(|left, right| compare_utf16(left, right));
            output.push(b'{');
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                output.extend_from_slice(serde_json::to_string(key)?.as_bytes());
                output.push(b':');
                encode_value(&values[key], output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn compare_utf16(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

pub fn domain_separated_bytes(domain: &'static str, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(domain.len() + 1 + payload.len());
    bytes.extend_from_slice(domain.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(payload);
    bytes
}

pub(crate) fn domain_hash(domain: &'static str, payload: &[u8]) -> blake3::Hash {
    blake3::hash(&domain_separated_bytes(domain, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_separator_is_exactly_one_nul_byte() {
        assert_eq!(
            domain_separated_bytes("layrs-test-v1", b"payload"),
            b"layrs-test-v1\0payload"
        );
    }

    #[test]
    fn byte_numbers_and_nested_combinations_match_the_general_jcs_formatter() {
        for byte in 0..=255u64 {
            let value = serde_json::json!(byte);
            assert_eq!(
                canonical_json(&value).unwrap(),
                serde_jcs::to_vec(&value).unwrap()
            );
        }
        let all: Vec<u8> = (0..=255).collect();
        let value = serde_json::json!({
            "z": all,
            "a": [{"bytes":all,"empty":[]}, [0,9,10,99,100,255], {"unicode":"é\n𝄞"}],
        });
        assert_eq!(
            canonical_json(&value).unwrap(),
            serde_jcs::to_vec(&value).unwrap()
        );
        for bytes in [
            "-0.0",
            "0.0",
            "1.0",
            "255.0",
            "256",
            "-1",
            "1.5",
            "1e30",
            "1e-30",
            "9007199254740993",
            "18446744073709551615",
        ] {
            let value: serde_json::Value = serde_json::from_str(bytes).unwrap();
            assert_eq!(
                canonical_json(&value).unwrap(),
                serde_jcs::to_vec(&value).unwrap(),
                "{bytes}"
            );
        }
    }
}
