//! A generic, namespaced document store with optional embeddings.
//!
//! This is a domain-agnostic key-value store keyed by `(collection, id)`. The
//! `body` is opaque JSON (this layer never interprets it) and an optional
//! little-endian `f32` embedding vector can be attached for semantic retrieval.
//!
//! `deepagent-memory` uses this for cross-session persistence and vector search
//! without `deepagent-persistence` needing to know anything about memory types
//! — keeping the dependency direction clean (memory -> persistence).

use deepagent_core::clock::Timestamp;
use deepagent_core::error::{CoreError, Result};
use rusqlite::{params, OptionalExtension};

use crate::{map_sqlite, Database};

/// A stored document.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// Caller-defined id (unique within its collection).
    pub id: String,
    /// Namespace.
    pub collection: String,
    /// Opaque JSON body.
    pub body: String,
    /// Optional embedding vector.
    pub embedding: Option<Vec<f32>>,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last-updated time.
    pub updated_at: Timestamp,
}

/// Repository over the `documents` table.
pub struct DocumentStore<'db> {
    db: &'db Database,
}

impl<'db> DocumentStore<'db> {
    /// Wrap a database handle.
    pub fn new(db: &'db Database) -> Self {
        Self { db }
    }

    /// Insert or replace a document (upsert), updating timestamps.
    pub fn put(
        &self,
        collection: &str,
        id: &str,
        body: &str,
        embedding: Option<&[f32]>,
        now: Timestamp,
    ) -> Result<()> {
        let blob = embedding.map(encode_vec);
        self.db.with_conn(|c| {
            c.execute(
                "INSERT INTO documents (id, collection, body, embedding, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(collection, id) DO UPDATE SET
                     body = excluded.body,
                     embedding = excluded.embedding,
                     updated_at = excluded.updated_at",
                params![id, collection, body, blob, now.as_millis()],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    /// Fetch a single document by collection + id.
    pub fn get(&self, collection: &str, id: &str) -> Result<Option<Document>> {
        self.db.with_conn(|c| {
            c.query_row(
                "SELECT id, collection, body, embedding, created_at, updated_at
                 FROM documents WHERE collection = ?1 AND id = ?2",
                params![collection, id],
                row_to_document,
            )
            .optional()
            .map_err(map_sqlite)
        })
    }

    /// List all documents in a collection (insertion-stable by created_at).
    pub fn list(&self, collection: &str) -> Result<Vec<Document>> {
        self.db.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, collection, body, embedding, created_at, updated_at
                     FROM documents WHERE collection = ?1 ORDER BY created_at ASC",
                )
                .map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![collection], row_to_document)
                .map_err(map_sqlite)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(map_sqlite)?);
            }
            Ok(out)
        })
    }

    /// Delete a document. Returns true if a row was removed.
    pub fn delete(&self, collection: &str, id: &str) -> Result<bool> {
        self.db.with_conn(|c| {
            let n = c
                .execute(
                    "DELETE FROM documents WHERE collection = ?1 AND id = ?2",
                    params![collection, id],
                )
                .map_err(map_sqlite)?;
            Ok(n > 0)
        })
    }

    /// Count documents in a collection.
    pub fn count(&self, collection: &str) -> Result<u64> {
        self.db.with_conn(|c| {
            let n: i64 = c
                .query_row(
                    "SELECT count(*) FROM documents WHERE collection = ?1",
                    params![collection],
                    |r| r.get(0),
                )
                .map_err(map_sqlite)?;
            Ok(n as u64)
        })
    }
}

fn row_to_document(row: &rusqlite::Row<'_>) -> rusqlite::Result<Document> {
    let id: String = row.get(0)?;
    let collection: String = row.get(1)?;
    let body: String = row.get(2)?;
    let blob: Option<Vec<u8>> = row.get(3)?;
    let created: i64 = row.get(4)?;
    let updated: i64 = row.get(5)?;
    Ok(Document {
        id,
        collection,
        body,
        embedding: blob.as_deref().map(decode_vec),
        created_at: Timestamp::from_millis(created),
        updated_at: Timestamp::from_millis(updated),
    })
}

/// Encode an `f32` slice as little-endian bytes.
fn encode_vec(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

/// Decode little-endian bytes back into an `f32` vector (trailing partial bytes
/// are ignored defensively).
fn decode_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Decode helper exposed for callers that read raw embeddings.
pub fn decode_embedding(bytes: &[u8]) -> Vec<f32> {
    decode_vec(bytes)
}

impl Database {
    /// Convenience: validate a document body parses as JSON before storing.
    /// Returns the error variant used elsewhere for serialization issues.
    pub fn validate_json(body: &str) -> Result<()> {
        serde_json::from_str::<serde_json::Value>(body)
            .map(|_| ())
            .map_err(|e| CoreError::Serialization(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    #[test]
    fn put_get_roundtrip_with_embedding() {
        let db = Database::open_in_memory().unwrap();
        let store = DocumentStore::new(&db);
        let emb = vec![0.1_f32, 0.2, 0.3];
        store
            .put("memory", "m1", r#"{"x":1}"#, Some(&emb), at(100))
            .unwrap();

        let doc = store.get("memory", "m1").unwrap().unwrap();
        assert_eq!(doc.body, r#"{"x":1}"#);
        let got = doc.embedding.unwrap();
        assert_eq!(got.len(), 3);
        assert!((got[1] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn upsert_updates_body() {
        let db = Database::open_in_memory().unwrap();
        let store = DocumentStore::new(&db);
        store.put("c", "id", r#"{"v":1}"#, None, at(1)).unwrap();
        store.put("c", "id", r#"{"v":2}"#, None, at(2)).unwrap();
        let doc = store.get("c", "id").unwrap().unwrap();
        assert_eq!(doc.body, r#"{"v":2}"#);
        assert_eq!(store.count("c").unwrap(), 1);
    }

    #[test]
    fn list_and_delete() {
        let db = Database::open_in_memory().unwrap();
        let store = DocumentStore::new(&db);
        store.put("c", "a", "{}", None, at(1)).unwrap();
        store.put("c", "b", "{}", None, at(2)).unwrap();
        assert_eq!(store.list("c").unwrap().len(), 2);
        assert!(store.delete("c", "a").unwrap());
        assert!(!store.delete("c", "a").unwrap());
        assert_eq!(store.count("c").unwrap(), 1);
    }

    #[test]
    fn collections_are_isolated() {
        let db = Database::open_in_memory().unwrap();
        let store = DocumentStore::new(&db);
        store.put("c1", "x", "{}", None, at(1)).unwrap();
        store.put("c2", "x", "{}", None, at(1)).unwrap();
        assert_eq!(store.count("c1").unwrap(), 1);
        assert_eq!(store.count("c2").unwrap(), 1);
        assert!(store.get("c1", "x").unwrap().is_some());
    }

    #[test]
    fn vec_codec_roundtrip() {
        let v = vec![1.0_f32, -2.5, 3.125, 0.0];
        let bytes = encode_vec(&v);
        assert_eq!(decode_vec(&bytes), v);
    }
}
