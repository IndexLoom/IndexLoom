use std::{
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use openidconnect::{
    ClientId, IssuerUrl, JsonWebKey, Nonce, PrivateSigningKey,
    core::{
        CoreIdToken, CoreIdTokenVerifier, CoreJsonWebKey, CoreJsonWebKeySet,
        CoreJwsSigningAlgorithm, CoreRsaPrivateSigningKey,
    },
};
use serde_json::{Value, json};

// Public upstream openidconnect test fixture, never an operational key.
const TEST_KEY: &str = include_str!("../fixtures/rsa-test-key.pem");
const MESSAGE: &[u8] = b"IndexLoom RSA security regression";

#[test]
fn upstream_rsa_known_vectors_preserve_jwk_compatibility() {
    // Independent signatures from upstream, including the RFC 7520 examples.
    let vectors: Value =
        serde_json::from_str(include_str!("../fixtures/rsa-jws-vectors.json")).unwrap();
    let jwk: CoreJsonWebKey = serde_json::from_value(vectors["jwk"].clone()).unwrap();
    for vector in vectors["cases"].as_array().unwrap() {
        let alg: CoreJwsSigningAlgorithm = serde_json::from_value(vector["alg"].clone()).unwrap();
        let message = vector["message"].as_str().unwrap().as_bytes();
        let signature = URL_SAFE_NO_PAD
            .decode(vector["signature"].as_str().unwrap())
            .unwrap();
        jwk.verify_signature(&alg, message, &signature).unwrap();

        // Leading-zero JWK integers remain accepted (RFC 7518 compatibility).
        let mut padded = vectors["jwk"].clone();
        let mut modulus = vec![0, 0];
        modulus.extend(
            URL_SAFE_NO_PAD
                .decode(padded["n"].as_str().unwrap())
                .unwrap(),
        );
        padded["n"] = json!(URL_SAFE_NO_PAD.encode(modulus));
        let padded: CoreJsonWebKey = serde_json::from_value(padded).unwrap();
        padded.verify_signature(&alg, message, &signature).unwrap();

        for (field, value) in [
            ("use", json!("enc")),
            ("alg", json!("HS256")),
            ("kty", json!("EC")),
        ] {
            let mut incompatible = vectors["jwk"].clone();
            incompatible[field] = value;
            let incompatible: CoreJsonWebKey = serde_json::from_value(incompatible).unwrap();
            assert!(
                incompatible
                    .verify_signature(&alg, message, &signature)
                    .is_err()
            );
        }
    }
}

fn signed_token(key: &CoreRsaPrivateSigningKey, claims: &Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256"}"#);
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
    let message = format!("{header}.{payload}");
    let signature = key
        .sign(
            &CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            message.as_bytes(),
        )
        .unwrap();
    format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature))
}

#[test]
fn all_rsa_jwt_algorithms_verify_and_reject_tampering() {
    let key = CoreRsaPrivateSigningKey::from_pem(TEST_KEY, None).unwrap();
    // Exercise the public JWK serialization boundary used by provider discovery.
    let jwk: CoreJsonWebKey =
        serde_json::from_value(serde_json::to_value(key.as_verification_key()).unwrap()).unwrap();
    for alg in [
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha384,
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha512,
        CoreJwsSigningAlgorithm::RsaSsaPssSha256,
        CoreJwsSigningAlgorithm::RsaSsaPssSha384,
        CoreJwsSigningAlgorithm::RsaSsaPssSha512,
    ] {
        let mut signature = key.sign(&alg, MESSAGE).unwrap();
        jwk.verify_signature(&alg, MESSAGE, &signature).unwrap();
        assert!(
            jwk.verify_signature(&alg, b"tampered message", &signature)
                .is_err()
        );
        signature[0] ^= 1;
        assert!(jwk.verify_signature(&alg, MESSAGE, &signature).is_err());
    }
    assert!(key.sign(&CoreJwsSigningAlgorithm::None, MESSAGE).is_err());
}

#[test]
fn rs256_id_tokens_require_valid_signature_issuer_audience_and_nonce() {
    let key = CoreRsaPrivateSigningKey::from_pem(TEST_KEY, None).unwrap();
    let jwk: CoreJsonWebKey =
        serde_json::from_value(serde_json::to_value(key.as_verification_key()).unwrap()).unwrap();
    let verifier = CoreIdTokenVerifier::new_public_client(
        ClientId::new("indexloom-test".into()),
        IssuerUrl::new("https://pocket-id.example".into()).unwrap(),
        CoreJsonWebKeySet::new(vec![jwk]),
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = json!({"iss":"https://pocket-id.example", "aud":"indexloom-test",
        "sub":"test-user", "iat":now, "exp":now + 300, "nonce":"test-nonce",
        "email":"admin@example.com", "email_verified":true});
    let token = signed_token(&key, &claims);
    let nonce = Nonce::new("test-nonce".into());
    CoreIdToken::from_str(&token)
        .unwrap()
        .claims(&verifier, &nonce)
        .unwrap();
    assert!(
        CoreIdToken::from_str(&token)
            .unwrap()
            .claims(&verifier, &Nonce::new("wrong".into()))
            .is_err()
    );
    for (field, invalid) in [
        ("iss", "https://attacker.example"),
        ("aud", "another-client"),
        ("nonce", "another-nonce"),
    ] {
        let mut invalid_claims = claims.clone();
        invalid_claims[field] = json!(invalid);
        let invalid_token = CoreIdToken::from_str(&signed_token(&key, &invalid_claims)).unwrap();
        assert!(
            invalid_token.claims(&verifier, &nonce).is_err(),
            "accepted invalid {field}"
        );
    }
    // Change the payload while retaining the original signature.
    let parts: Vec<_> = token.split('.').collect();
    let mut forged = claims;
    forged["sub"] = json!("attacker");
    let tampered = format!(
        "{}.{}.{}",
        parts[0],
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged).unwrap()),
        parts[2]
    );
    assert!(
        CoreIdToken::from_str(&tampered)
            .unwrap()
            .claims(&verifier, &nonce)
            .is_err()
    );
    let mut signature = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
    signature[0] ^= 1;
    let tampered = format!(
        "{}.{}.{}",
        parts[0],
        parts[1],
        URL_SAFE_NO_PAD.encode(signature)
    );
    assert!(
        CoreIdToken::from_str(&tampered)
            .unwrap()
            .claims(&verifier, &nonce)
            .is_err()
    );
}
