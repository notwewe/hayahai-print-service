use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).expect("strings serialize"),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("keys serialize"),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

pub fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_millis() as u64
}

pub fn signature(
    key: &SigningKey,
    method: &str,
    path: &str,
    timestamp: u64,
    counter: u64,
    body: &Value,
) -> String {
    let digest = hex::encode(Sha256::digest(canonical_json(body).as_bytes()));
    let payload = format!(
        "v1\n{}\n{}\n{}\n{}\n{}",
        method.to_uppercase(),
        path,
        timestamp,
        counter,
        digest
    );
    STANDARD.encode(key.sign(payload.as_bytes()).to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_sorts_nested_keys() {
        assert_eq!(
            canonical_json(&json!({"z": 1, "a": {"d": true, "b": null}})),
            r#"{"a":{"b":null,"d":true},"z":1}"#
        );
    }

    #[test]
    fn signature_matches_the_client_api_protocol_vector() {
        let key = SigningKey::from_bytes(&[1_u8; 32]);
        let body = json!({
            "version": "1.0.0",
            "printers": [{
                "queueId": "printer",
                "available": true,
                "capabilities": { "provider": "cups", "raw": true }
            }]
        });
        assert_eq!(
            signature(
                &key,
                "POST",
                "/printing/agent/heartbeat",
                1_727_000_000_000,
                42,
                &body,
            ),
            "UWgeqD7ij+xJK+pm3ZmCLWSpHrQZoKXbdxe7EuRI7HQFLu6n69DiJq5jmVu6/E1fQiWvSkO2ED2HgE+QIYwyCQ=="
        );
    }
}
