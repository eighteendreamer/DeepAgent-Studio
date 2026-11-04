//! Integrity-protected, bounded history coordinates, not an authorization token.
//! A pagination-only random key lives in the existing DocumentStore. This never
//! opens the credential store or creates a second event/history persistence path.

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::bounded_read::json_len;
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;
use serde::{de::DeserializeOwned, Serialize};

const COLLECTION: &str = "harness.history.internal";
const KEY_ID: &str = "cursor-key-v1";
const PREFIX: &str = "h1.";
const MAX_STATE_BYTES: usize = 3000;
pub(super) const MAX_TOKEN_BYTES: usize = 4096;

/// Exact JSON-string payload length: this version uses only unescaped ASCII.
/// Counting must not generate random tokens or mutate the key record.
pub(super) fn encoded_len(state: &impl Serialize) -> Result<usize> {
    let plaintext = json_len(state, MAX_STATE_BYTES).map_err(|_| invalid_cursor())?;
    Ok(PREFIX.len() + ((plaintext + 12 + 16) * 4).div_ceil(3))
}

pub(super) fn encode<T: Serialize>(db: &Database, thread: &str, state: &T) -> Result<String> {
    // Reject before both allocation and key creation. No persisted cursor list
    // or unbounded server-side replay cache is needed for read-only retries.
    json_len(state, MAX_STATE_BYTES).map_err(|_| invalid_cursor())?;
    let plaintext = serde_json::to_vec(state).map_err(|_| invalid_cursor())?;
    let key = key(db, true)?.ok_or_else(corrupt_key)?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = Aes256Gcm::new(&key)
        .encrypt(
            &nonce,
            Payload {
                msg: &plaintext,
                aad: &aad(thread),
            },
        )
        .map_err(|_| CoreError::other("failed to encode history continuation"))?;
    let mut bytes = Vec::with_capacity(nonce.len() + ciphertext.len());
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&ciphertext);
    let token = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    if token.len() > MAX_TOKEN_BYTES {
        return Err(invalid_cursor());
    }
    Ok(token)
}

pub(super) fn decode<T: DeserializeOwned>(db: &Database, thread: &str, token: &str) -> Result<T> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(invalid_cursor());
    }
    let payload = token.strip_prefix(PREFIX).ok_or_else(invalid_cursor)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| invalid_cursor())?;
    if bytes.len() < 12 + 16 || bytes.len() > MAX_STATE_BYTES + 12 + 16 {
        return Err(invalid_cursor());
    }
    // Unknown/malformed continuations never create a key or access credentials.
    let key = key(db, false)?.ok_or_else(invalid_cursor)?;
    let plaintext = Aes256Gcm::new(&key)
        .decrypt(
            Nonce::from_slice(&bytes[..12]),
            Payload {
                msg: &bytes[12..],
                aad: &aad(thread),
            },
        )
        .map_err(|_| invalid_cursor())?;
    serde_json::from_slice(&plaintext).map_err(|_| invalid_cursor())
}

fn key(db: &Database, create: bool) -> Result<Option<Key<Aes256Gcm>>> {
    let store = DocumentStore::new(db);
    let mut body = key_body(&store)?;
    if body.is_none() && create {
        let generated = Aes256Gcm::generate_key(&mut OsRng);
        store.insert_if_absent(
            COLLECTION,
            KEY_ID,
            &URL_SAFE_NO_PAD.encode(generated),
            SystemClock.now(),
        )?;
        body = key_body(&store)?;
    }
    body.map(|body| {
        let bytes = URL_SAFE_NO_PAD.decode(body).map_err(|_| corrupt_key())?;
        if bytes.len() != 32 {
            return Err(corrupt_key());
        }
        Ok(*Key::<Aes256Gcm>::from_slice(&bytes))
    })
    .transpose()
}

fn key_body(store: &DocumentStore<'_>) -> Result<Option<String>> {
    store
        .get_body_bounded(COLLECTION, KEY_ID, 64)
        .map_err(|error| match error {
            CoreError::Invalid(_) => corrupt_key(),
            other => CoreError::Persistence(format!("read history continuation key: {other}")),
        })
}

fn aad(thread: &str) -> Vec<u8> {
    let mut value = b"deepagent.harness.thread/read.history.v1\0".to_vec();
    value.extend_from_slice(thread.as_bytes());
    value
}

fn invalid_cursor() -> CoreError {
    CoreError::invalid("invalid or foreign history continuation")
}

fn corrupt_key() -> CoreError {
    CoreError::Persistence("history continuation key is unavailable or corrupt".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::Timestamp;
    use deepagent_persistence::document_store::DocumentStore;
    use serde_json::{json, Value};

    #[test]
    fn h218_cursor_roundtrip_reopen_and_retry_without_credential_access() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cursor.sqlite");
        let db = Database::open(&path).unwrap();
        let state = json!({"position":7,"private":"synthetic-only"});
        let token = encode(&db, "thread-a", &state).unwrap();
        assert_eq!(token.len(), encoded_len(&state).unwrap());
        assert!(token.len() <= 4096);
        assert!(!token.contains("synthetic-only"));
        drop(db);
        let db = Database::open(&path).unwrap();
        for _ in 0..2 {
            assert_eq!(decode::<Value>(&db, "thread-a", &token).unwrap(), state);
        }
        assert_eq!(DocumentStore::new(&db).count(COLLECTION).unwrap(), 1);
    }

    #[test]
    fn h218_cursor_tamper_foreign_database_and_thread_are_rejected() {
        let db = Database::open_in_memory().unwrap();
        let token = encode(&db, "thread-a", &json!({"position":7})).unwrap();
        assert!(decode::<Value>(&db, "thread-b", &token).is_err());
        let other = Database::open_in_memory().unwrap();
        assert!(decode::<Value>(&other, "thread-a", &token).is_err());
        assert_eq!(DocumentStore::new(&other).count(COLLECTION).unwrap(), 0);
        for index in [5, token.len() / 2, token.len() - 2] {
            let mut changed = token.as_bytes().to_vec();
            changed[index] = if changed[index] == b'A' { b'B' } else { b'A' };
            assert!(
                decode::<Value>(&db, "thread-a", std::str::from_utf8(&changed).unwrap()).is_err()
            );
        }
        assert!(decode::<Value>(&db, "thread-a", &(token + "x")).is_err());
    }

    #[test]
    fn h218_cursor_bad_inputs_are_bounded_and_do_not_create_or_replace_key() {
        let db = Database::open_in_memory().unwrap();
        let store = DocumentStore::new(&db);
        for input in [
            String::new(),
            "bad".into(),
            "h1.!!!".into(),
            "x".repeat(4097),
        ] {
            assert!(decode::<Value>(&db, "thread-a", &input).is_err());
        }
        assert!(encode(
            &db,
            "thread-a",
            &json!({"x":"synthetic-private".repeat(4096)})
        )
        .is_err());
        assert_eq!(store.count(COLLECTION).unwrap(), 0);
        for body in ["broken-key".to_owned(), "private-key-marker".repeat(4096)] {
            store
                .put(COLLECTION, KEY_ID, &body, None, Timestamp::from_millis(1))
                .unwrap();
            let error = encode(&db, "thread-a", &json!({"position":1})).unwrap_err();
            assert!(!error.to_string().contains("private-key-marker"));
            assert_eq!(store.get(COLLECTION, KEY_ID).unwrap().unwrap().body, body);
        }
    }

    #[test]
    fn h218_cursor_concurrent_creators_share_one_key_across_connections() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("race.sqlite");
        let first = Database::open(&path).unwrap();
        let second = Database::open(&path).unwrap();
        let gate = std::sync::Barrier::new(2);
        let tokens = std::thread::scope(|scope| {
            let a = scope.spawn(|| {
                gate.wait();
                encode(&first, "thread-a", &json!(1)).unwrap()
            });
            let b = scope.spawn(|| {
                gate.wait();
                encode(&second, "thread-a", &json!(2)).unwrap()
            });
            [a.join().unwrap(), b.join().unwrap()]
        });
        assert_eq!(
            decode::<Value>(&second, "thread-a", &tokens[0]).unwrap(),
            json!(1)
        );
        assert_eq!(
            decode::<Value>(&first, "thread-a", &tokens[1]).unwrap(),
            json!(2)
        );
        assert_eq!(DocumentStore::new(&first).count(COLLECTION).unwrap(), 1);
    }
}
