use crate::error::{APWError, Result};
use crate::types::{MSGTypes, PAKEMessage, SecretSessionVersion, Status};
use crate::utils::{mod_, pad, powermod, sha256};
use base64::{engine::general_purpose, Engine as _};
use hex::FromHex;
use num_bigint::BigUint;
use num_traits::Zero;
use openssl::symm::{Cipher, Crypter, Mode};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use subtle::ConstantTimeEq;

const GROUP_PRIME_BYTES: usize = 384;
const GROUP_GENERATOR: u8 = 5;
const DEFAULT_VERSION: &str = "1.0.1";
const NONCE_BYTES: usize = 16;
const GCM_TAG_BYTES: usize = 16;

const GROUP_PRIME: &str = "\
FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514087098E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE45B3DC2007CB8A163BF0598DA48361C55D39A69163FA8FD24CF5F83655D23DCA3AD96E1C62F356208552BB9ED529077096966D670C354E4ABC9804F1746C08CA18217C32905E462E36CE3BE39E772C180E86039B2783A2EC07A28FB5C55DF06F4C52C9DE2BCB695817183995497CEA956AE515D2261898FA0510F15728E5A8AAC42DAD33170D04507A33A85521ABD1CBA64ECFB850458DBEF0A8AEA71575D060C7DB3970F85A6E1E4C7ABF5AE8CDB0933D71C94C4A25619DCEE3E2261AD2EE6BF12FFA06D98A0864D87602733EC86A64521F2B18177B200CBBE117577615D6C770988CBD946E208E24FA074E5AB3143DB5BFE0FD108E4B82D120A93AD2CAFFFFFFFFFFFFFFFF";

fn group_prime() -> BigUint {
    BigUint::parse_bytes(GROUP_PRIME.as_bytes(), 16).expect("group prime")
}

fn multiplier() -> BigUint {
    let prime = group_prime();
    let hash = sha256(
        &[
            pad(&prime.to_bytes_be(), GROUP_PRIME_BYTES),
            pad(&[GROUP_GENERATOR], GROUP_PRIME_BYTES),
        ]
        .concat(),
    );
    BigUint::from_bytes_be(&hash)
}

fn parse_numeric_token(value: &Value) -> Option<i64> {
    match value {
        Value::Number(value) => value.as_i64(),
        Value::String(value) => value.parse::<i64>().ok(),
        Value::Array(values) => values.first().and_then(parse_numeric_token),
        _ => None,
    }
}

#[allow(dead_code)]
pub fn parse_pake_message_code(value: &Value) -> Option<i64> {
    parse_numeric_token(value)
}

pub fn parse_pake_message_type(value: &Value) -> Option<i64> {
    parse_numeric_token(value)
}

pub fn parse_pake_message_to_struct(value: &Value) -> Result<PAKEMessage> {
    serde_json::from_value(value.clone()).map_err(|_| {
        APWError::new(
            Status::ProtoInvalidResponse,
            "Invalid PAKE message payload.",
        )
    })
}

pub fn is_valid_pake_message(candidate: &Value) -> bool {
    let message = match parse_pake_message_to_struct(candidate) {
        Ok(value) => value,
        Err(_) => return false,
    };

    if message.TID.trim().is_empty() {
        return false;
    }
    if parse_numeric_token(&message.MSG).is_none() {
        return false;
    }
    if message.A.trim().is_empty() || message.s.trim().is_empty() || message.B.trim().is_empty() {
        return false;
    }
    if parse_numeric_token(&message.PROTO).is_none() {
        return false;
    }
    if let Some(value) = message.HAMK.as_ref() {
        if value.trim().is_empty() {
            return false;
        }
    }
    if let Some(value) = message.ErrCode.as_ref() {
        if parse_numeric_token(value).is_none() {
            return false;
        }
    }
    if let Some(value) = message.VER.as_ref() {
        match value {
            Value::String(v) if v.is_empty() => return false,
            Value::Number(v) if !v.is_i64() => return false,
            _ => {}
        }
    }

    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionValues {
    pub username: Option<String>,
    pub shared_key: Option<BigUint>,
    pub client_private_key: Option<BigUint>,
    pub salt: Option<BigUint>,
    pub server_public_key: Option<BigUint>,
}

#[derive(Debug, Clone)]
pub struct SRPSession {
    pub username: String,
    client_private_key: BigUint,
    server_public_key: Option<BigUint>,
    salt: Option<BigUint>,
    shared_key: Option<BigUint>,
    should_use_base64: bool,
}

impl SRPSession {
    pub fn new(should_use_base64: bool) -> Self {
        let mut seed = crate::utils::random_bytes(32);
        let username = crate::utils::random_bytes(16);
        let client_private_key = BigUint::from_bytes_be(&seed);
        rand::thread_rng().fill_bytes(&mut seed);
        Self {
            username: if should_use_base64 {
                general_purpose::STANDARD.encode(username)
            } else {
                format!("0x{}", hex::encode(&username))
            },
            client_private_key,
            server_public_key: None,
            salt: None,
            shared_key: None,
            should_use_base64,
        }
    }

    pub fn return_values(&self) -> SessionValues {
        SessionValues {
            username: Some(self.username.clone()),
            shared_key: self.shared_key.clone(),
            client_private_key: Some(self.client_private_key.clone()),
            salt: self.salt.clone(),
            server_public_key: self.server_public_key.clone(),
        }
    }

    pub fn shared_key(&self) -> Option<&BigUint> {
        self.shared_key.as_ref()
    }

    pub fn update_with_values(&mut self, values: SessionValues) {
        if let Some(username) = values.username {
            self.username = username;
        }
        if let Some(shared_key) = values.shared_key {
            self.shared_key = Some(shared_key);
        }
        if let Some(client_private_key) = values.client_private_key {
            self.client_private_key = client_private_key;
        }
        if let Some(salt) = values.salt {
            self.salt = Some(salt);
        }
        if let Some(server_public_key) = values.server_public_key {
            self.server_public_key = Some(server_public_key);
        }
    }

    pub fn client_public_key(&self) -> BigUint {
        powermod(
            &BigUint::from(GROUP_GENERATOR),
            &self.client_private_key,
            &group_prime(),
        )
        .expect("client public key")
    }

    fn derive_scramble(&self) -> Vec<u8> {
        let server_public_key = self
            .server_public_key
            .as_ref()
            .expect("invalid session state: missing server public key");
        sha256(
            &[
                pad(&self.client_public_key().to_bytes_be(), GROUP_PRIME_BYTES),
                pad(&server_public_key.to_bytes_be(), GROUP_PRIME_BYTES),
            ]
            .concat(),
        )
    }

    fn derive_session_key(&self, password: &str) -> Result<BigUint> {
        let server_public_key = self.server_public_key.as_ref().ok_or_else(|| {
            APWError::new(
                Status::InvalidSession,
                "Invalid session state: missing server values",
            )
        })?;
        let salt = self.salt.as_ref().ok_or_else(|| {
            APWError::new(
                Status::InvalidSession,
                "Invalid session state: missing session values",
            )
        })?;

        let username_password_hash = sha256(format!("{}:{}", self.username, password).as_bytes());
        let salted_password_hash = sha256(
            &[
                pad(&salt.to_bytes_be(), GROUP_PRIME_BYTES),
                username_password_hash,
            ]
            .concat(),
        );
        let x = BigUint::from_bytes_be(&salted_password_hash);
        let u = BigUint::from_bytes_be(&self.derive_scramble());
        let k = multiplier();
        let gx = powermod(&BigUint::from(GROUP_GENERATOR), &x, &group_prime())?;
        let base = {
            let modulus = group_prime();
            let rhs = mod_(&(k * gx), &modulus);
            if server_public_key >= &rhs {
                mod_(&(server_public_key - &rhs), &modulus)
            } else {
                let wrapped = server_public_key + &modulus;
                mod_(&(wrapped - &rhs), &modulus)
            }
        };
        if base.is_zero() {
            return Err(APWError::new(
                Status::ServerError,
                "Invalid server hello: invalid public key",
            ));
        }

        let exponent = self.client_private_key.clone() + &u * x;
        let shared_secret = powermod(&base, &exponent, &group_prime())?;
        Ok(BigUint::from_bytes_be(&sha256(
            &shared_secret.to_bytes_be(),
        )))
    }

    pub fn set_server_public_key(
        &mut self,
        server_public_key: BigUint,
        salt: BigUint,
    ) -> Result<()> {
        if mod_(&server_public_key, &group_prime()).is_zero() {
            return Err(APWError::new(
                Status::InvalidSession,
                "Invalid server hello: invalid public key",
            ));
        }
        self.server_public_key = Some(server_public_key);
        self.salt = Some(salt);
        Ok(())
    }

    pub fn set_shared_key(&mut self, password: &str) -> Result<BigUint> {
        if self.server_public_key.is_none() || self.salt.is_none() {
            return Err(APWError::new(
                Status::InvalidSession,
                "Invalid session state: missing handshake values",
            ));
        }
        let shared_key = self.derive_session_key(password)?;
        self.shared_key = Some(shared_key.clone());
        Ok(shared_key)
    }

    pub fn compute_m(&self) -> Result<Vec<u8>> {
        let salt = self.salt.as_ref().ok_or_else(|| {
            APWError::new(
                Status::InvalidSession,
                "Invalid session state: missing salt",
            )
        })?;
        let server_public_key = self.server_public_key.as_ref().ok_or_else(|| {
            APWError::new(
                Status::InvalidSession,
                "Invalid session state: missing server key",
            )
        })?;
        let shared_key = self.shared_key.as_ref().ok_or_else(|| {
            APWError::new(
                Status::InvalidSession,
                "Missing encryption key. Use `apw app launch` and `apw login <url>` through the native app broker.",
            )
        })?;

        let n_hash = sha256(&pad(&group_prime().to_bytes_be(), GROUP_PRIME_BYTES));
        let g_hash = sha256(&pad(&[GROUP_GENERATOR], GROUP_PRIME_BYTES));
        let xored = n_hash
            .into_iter()
            .zip(g_hash)
            .map(|(l, r)| l ^ r)
            .collect::<Vec<_>>();
        let scramble = self.derive_scramble();

        Ok(sha256(
            &[
                xored,
                sha256(self.username.as_bytes()),
                pad(&salt.to_bytes_be(), GROUP_PRIME_BYTES),
                pad(&self.client_public_key().to_bytes_be(), GROUP_PRIME_BYTES),
                pad(&server_public_key.to_bytes_be(), GROUP_PRIME_BYTES),
                pad(&shared_key.to_bytes_be(), GROUP_PRIME_BYTES),
                scramble,
            ]
            .concat(),
        ))
    }

    pub fn compute_hmac(&self, proof: &[u8]) -> Result<Vec<u8>> {
        let shared_key = self.shared_key.as_ref().ok_or_else(|| {
            APWError::new(
                Status::InvalidSession,
                "Missing encryption key. Use `apw app launch` and `apw login <url>` through the native app broker.",
            )
        })?;

        let mut payload = self.client_public_key().to_bytes_be();
        payload.extend_from_slice(proof);
        payload.extend_from_slice(&shared_key.to_bytes_be());
        Ok(sha256(&payload))
    }

    pub fn serialize(&self, input: &[u8], prefix: bool) -> String {
        if self.should_use_base64 {
            general_purpose::STANDARD.encode(input)
        } else {
            let encoded = hex::encode(input);
            if prefix {
                format!("0x{encoded}")
            } else {
                encoded
            }
        }
    }

    pub fn deserialize(&self, input: &str) -> Result<Vec<u8>> {
        if self.should_use_base64 {
            decode_with_fallback(input, true, "Invalid base64 value.")
        } else {
            decode_with_fallback(input, false, "Invalid hex value.")
        }
    }

    pub fn encrypt<T: serde::Serialize>(&self, value: &T) -> Result<Vec<u8>> {
        let shared_key = self.shared_key.as_ref().ok_or_else(|| {
            APWError::new(
                Status::InvalidSession,
                "Missing encryption key. Use `apw app launch` and `apw login <url>` through the native app broker.",
            )
        })?;
        let key = shared_key.to_bytes_be();
        if key.len() < 16 {
            return Err(APWError::new(Status::ServerError, "Shared key too short."));
        }

        let plain = serde_json::to_vec(value).map_err(|error| {
            APWError::new(
                Status::ServerError,
                format!("Payload serialization failed: {error}"),
            )
        })?;

        let mut nonce = [0_u8; NONCE_BYTES];
        rand::thread_rng().fill_bytes(&mut nonce);
        let mut crypter = Crypter::new(
            Cipher::aes_128_gcm(),
            Mode::Encrypt,
            &key[..16],
            Some(&nonce),
        )
        .map_err(|_| APWError::new(Status::ServerError, "Invalid encryption key."))?;

        let mut encrypted = vec![0_u8; plain.len() + GCM_TAG_BYTES];
        let count = crypter
            .update(&plain, &mut encrypted)
            .map_err(|_| APWError::new(Status::ServerError, "Encryption failed."))?;
        let finalize_count = crypter
            .finalize(&mut encrypted[count..])
            .map_err(|_| APWError::new(Status::ServerError, "Encryption failed."))?;
        encrypted.truncate(count + finalize_count);

        let mut tag = [0_u8; GCM_TAG_BYTES];
        crypter
            .get_tag(&mut tag)
            .map_err(|_| APWError::new(Status::ServerError, "Encryption failed."))?;
        encrypted.extend_from_slice(&tag);

        Ok([nonce.to_vec(), encrypted].concat())
    }

    pub fn decrypt(&self, payload: &[u8]) -> Result<Vec<u8>> {
        let shared_key = self.shared_key.as_ref().ok_or_else(|| {
            APWError::new(
                Status::InvalidSession,
                "Missing encryption key. Use `apw app launch` and `apw login <url>` through the native app broker.",
            )
        })?;
        if payload.len() <= NONCE_BYTES {
            return Err(APWError::new(
                Status::ProtoInvalidResponse,
                "Invalid server response payload.",
            ));
        }

        let key = shared_key.to_bytes_be();
        if key.len() < 16 {
            return Err(APWError::new(Status::ServerError, "Shared key too short."));
        }
        if payload.len() <= NONCE_BYTES + GCM_TAG_BYTES {
            return Err(APWError::new(
                Status::ProtoInvalidResponse,
                "Invalid server response payload.",
            ));
        }
        let (nonce, body) = payload.split_at(NONCE_BYTES);
        let (ciphertext, tag) = body.split_at(body.len() - GCM_TAG_BYTES);

        let mut crypter = Crypter::new(
            Cipher::aes_128_gcm(),
            Mode::Decrypt,
            &key[..16],
            Some(nonce),
        )
        .map_err(|_| APWError::new(Status::ServerError, "Invalid encryption key."))?;
        crypter
            .set_tag(tag)
            .map_err(|_| APWError::new(Status::ServerError, "Invalid encryption key."))?;

        let mut plain = vec![0_u8; ciphertext.len() + GCM_TAG_BYTES];
        let count = crypter.update(ciphertext, &mut plain).map_err(|_| {
            APWError::new(Status::ProtoInvalidResponse, "Failed to decrypt payload.")
        })?;
        let finalize_count = crypter.finalize(&mut plain[count..]).map_err(|_| {
            APWError::new(Status::ProtoInvalidResponse, "Failed to decrypt payload.")
        })?;
        plain.truncate(count + finalize_count);

        Ok(plain)
    }

    pub fn verify_hamk(&self, expected: &[u8], actual: &[u8]) -> bool {
        if expected.is_empty() || actual.is_empty() || expected.len() != actual.len() {
            return false;
        }
        ConstantTimeEq::ct_eq(expected, actual).into()
    }
}

impl Default for SRPSession {
    fn default() -> Self {
        Self::new(true)
    }
}

pub fn build_client_key_exchange(session: &SRPSession) -> Value {
    json!({
      "TID": session.username,
      "MSG": MSGTypes::ClientKeyExchange as i32,
      "A": session.serialize(&session.client_public_key().to_bytes_be(), true),
      "VER": DEFAULT_VERSION,
      "PROTO": [SecretSessionVersion::SrpWithRfcVerification as i32],
    })
}

pub fn build_client_verification_message(session: &SRPSession, proof: &[u8]) -> Value {
    json!({
      "TID": session.username,
      "MSG": MSGTypes::ClientVerification as i32,
      "M": session.serialize(proof, false),
    })
}

fn decode_with_fallback(input: &str, as_base64: bool, message: &str) -> Result<Vec<u8>> {
    if as_base64 {
        if let Ok(bytes) = general_purpose::STANDARD.decode(input) {
            return Ok(bytes);
        }
        let normalized = input.strip_prefix("0x").unwrap_or(input);
        return <Vec<u8> as FromHex>::from_hex(normalized)
            .map_err(|_| APWError::new(Status::ProtoInvalidResponse, message));
    }

    let normalized = input.strip_prefix("0x").unwrap_or(input);
    if let Ok(bytes) = <Vec<u8> as FromHex>::from_hex(normalized) {
        return Ok(bytes);
    }
    general_purpose::STANDARD
        .decode(input)
        .map_err(|_| APWError::new(Status::ProtoInvalidResponse, message))
}

pub fn decode_base64_or_hex(input: &str, as_base64: bool) -> Result<BigUint> {
    let bytes = decode_with_fallback(input, as_base64, "Invalid bigint encoding.")?;
    Ok(BigUint::from_bytes_be(&bytes))
}

pub use decode_base64_or_hex as base64_decode_numeric;

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{thread_rng, Rng, RngCore};

    #[test]
    fn valid_pake_message_is_accepted() {
        let message = serde_json::json!({
          "TID": "user",
          "MSG": MSGTypes::ServerVerification as i64,
          "A": "1",
          "s": "1",
          "B": "1",
          "PROTO": 1,
          "VER": 1,
        });

        assert!(is_valid_pake_message(&message));
    }

    #[test]
    fn invalid_pake_missing_fields_is_rejected() {
        let message = serde_json::json!({
          "TID": "",
          "MSG": "1",
          "A": "abc",
          "s": "abc",
          "B": "abc",
          "PROTO": 1,
        });
        assert!(!is_valid_pake_message(&message));
    }

    #[test]
    fn hamk_verification_is_constant_time_style_guarded() {
        let session = SRPSession::new(true);
        assert!(session.verify_hamk(&[1, 2, 3], &[1, 2, 3]));
        assert!(!session.verify_hamk(&[1, 2, 3], &[1, 2, 4]));
        assert!(!session.verify_hamk(&[], &[1, 2, 3]));
    }

    #[test]
    fn decode_base64_or_hex_roundtrip_property() {
        let mut rng = thread_rng();
        for _ in 0..512 {
            let len = rng.gen_range(0..256usize);
            let mut bytes = vec![0_u8; len];
            rng.fill_bytes(&mut bytes);

            let encoded_base64 = hex::encode(&bytes);
            let decoded_hex = decode_base64_or_hex(&format!("0x{encoded_base64}"), false).unwrap();
            assert_eq!(
                decoded_hex.to_bytes_be(),
                BigUint::from_bytes_be(&bytes).to_bytes_be()
            );

            let encoded_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let decoded_base64 = decode_base64_or_hex(&encoded_base64, true).unwrap();
            assert_eq!(
                decoded_base64.to_bytes_be(),
                BigUint::from_bytes_be(&bytes).to_bytes_be()
            );
        }
    }

    #[test]
    fn is_valid_pake_message_rejects_mutated_inputs() {
        let mut rng = thread_rng();
        for _ in 0..2048 {
            let base = json!({
              "TID": "alice",
              "MSG": MSGTypes::ServerVerification as i64,
              "A": "AQ==",
              "s": "AQ==",
              "B": "AQ==",
              "PROTO": [SecretSessionVersion::SrpWithRfcVerification as i64],
              "VER": "1",
              "ErrCode": 0,
              "HAMK": "AQ==",
            });

            let mut candidate = base;
            match rng.gen_range(0..8) {
                0 => candidate["TID"] = json!(""),
                1 => candidate["MSG"] = json!([]),
                2 => {
                    candidate.as_object_mut().unwrap().remove("A");
                }
                3 => candidate["PROTO"] = json!(["bad"]),
                4 => candidate["ErrCode"] = json!("bad"),
                5 => candidate["A"] = json!(""),
                6 => candidate["VER"] = Value::String(String::new()),
                _ => candidate["HAMK"] = json!(""),
            }

            assert!(
                !is_valid_pake_message(&candidate),
                "candidate must be rejected: {candidate}"
            );
        }
    }
}
