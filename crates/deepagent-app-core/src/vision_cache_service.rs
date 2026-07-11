//! Persistent cache for third-party image recognition results.

use std::fs;
use std::path::PathBuf;

use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

use crate::vision_provider_service::hash_bytes;

#[derive(Debug, Clone)]
pub struct VisionCacheService {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionCacheEntry {
    pub image_hash: String,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub prompt_version: String,
    pub result: String,
    pub raw_json: String,
    pub created_at_ms: i64,
}

impl VisionCacheService {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn key_for(
        &self,
        image_hash: &str,
        provider: &str,
        base_url: &str,
        model: &str,
        prompt_version: &str,
        prompt: &str,
    ) -> String {
        let material = [
            image_hash,
            provider,
            base_url.trim_end_matches('/'),
            model,
            prompt_version,
            prompt,
        ]
        .join("\n");
        hash_bytes(material.as_bytes())
    }

    pub fn get(&self, key: &str) -> Result<Option<VisionCacheEntry>> {
        let path = self.path_for(key);
        if !path.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)
            .map_err(|e| CoreError::Other(format!("read vision cache: {e}")))?;
        let entry = serde_json::from_str(&text)
            .map_err(|e| CoreError::Other(format!("parse vision cache: {e}")))?;
        Ok(Some(entry))
    }

    pub fn put(&self, key: &str, entry: &VisionCacheEntry) -> Result<()> {
        fs::create_dir_all(&self.root)
            .map_err(|e| CoreError::Other(format!("create vision cache dir: {e}")))?;
        let json = serde_json::to_string_pretty(entry)
            .map_err(|e| CoreError::Other(format!("serialize vision cache: {e}")))?;
        fs::write(self.path_for(key), json)
            .map_err(|e| CoreError::Other(format!("write vision cache: {e}")))?;
        Ok(())
    }

    fn path_for(&self, key: &str) -> PathBuf {
        self.root.join(format!("{}.json", sanitize_key(key)))
    }
}

fn sanitize_key(key: &str) -> String {
    key.chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .take(96)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = VisionCacheService::new(dir.path().to_path_buf());
        let key = cache.key_for("abc", "modelscope", "https://x/v1", "m", "v1", "prompt");
        let entry = VisionCacheEntry {
            image_hash: "abc".to_string(),
            provider: "modelscope".to_string(),
            base_url: "https://x/v1".to_string(),
            model: "m".to_string(),
            prompt_version: "v1".to_string(),
            result: "ok".to_string(),
            raw_json: "{}".to_string(),
            created_at_ms: 1,
        };
        cache.put(&key, &entry).unwrap();
        assert_eq!(cache.get(&key).unwrap().unwrap().result, "ok");
    }
}
