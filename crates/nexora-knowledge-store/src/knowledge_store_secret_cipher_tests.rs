use super::{
    knowledge_store_secret_cipher::open_openai_secret,
    provider_secret_envelope::ProviderSecretEnvelope,
};
use serde_json::json;

#[test]
fn decrypts_openai_secret_envelope_using_the_api_compatible_format() {
    let settings = json!({
        "endpoint": "https://api.openai.com/v1",
        "model": "gpt-4o",
        "embedding_model": "text-embedding-3-small",
        "effort": "medium",
        "enabled": true
    });
    let envelope = ProviderSecretEnvelope {
        key_version: 1,
        nonce: [2; 12],
        ciphertext: decode_hex(
            "5962e49c2633ecd06559382ceba74aadc3edb6bb61a2f7ec65875682d4b157dc1d6b2282",
        ),
    };

    let secret = open_openai_secret(&[7; 32], "nexora_platform", &settings, 1, &envelope).unwrap();

    assert_eq!(secret.as_slice(), b"fixture-openai-token");
}

#[test]
fn rejects_changed_openai_endpoint_or_secret_revision() {
    let settings = json!({
        "endpoint": "https://other.example.com/v1",
        "model": "gpt-4o",
        "embedding_model": "text-embedding-3-small",
        "effort": "medium",
        "enabled": true
    });
    let envelope = ProviderSecretEnvelope {
        key_version: 1,
        nonce: [2; 12],
        ciphertext: decode_hex(
            "5962e49c2633ecd06559382ceba74aadc3edb6bb61a2f7ec65875682d4b157dc1d6b2282",
        ),
    };

    assert!(open_openai_secret(&[7; 32], "nexora_platform", &settings, 1, &envelope).is_err());
    assert!(
        open_openai_secret(
            &[7; 32],
            "nexora_platform",
            &json!({
                "endpoint": "https://api.openai.com/v1",
                "model": "gpt-4o",
                "embedding_model": "text-embedding-3-small",
                "effort": "medium",
                "enabled": true
            }),
            2,
            &envelope
        )
        .is_err()
    );
}

fn decode_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
