use std::collections::HashSet;

use url::Url;

use super::types::{EngineError, EngineId, SearchErrorCode, SearchResult};

pub const MAX_ENGINE_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

pub struct NormalizedResult {
    pub identity: String,
    pub rank: usize,
    pub result: SearchResult,
}

pub fn result_identity(raw: &str) -> Option<String> {
    let mut url = Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    url.set_fragment(None);
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| {
            let key = key.to_ascii_lowercase();
            !key.starts_with("utm_") && !matches!(key.as_str(), "gclid" | "fbclid" | "msclkid")
        })
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    if !kept.is_empty() {
        url.query_pairs_mut().extend_pairs(kept);
    }
    Some(url.to_string())
}

pub fn normalize_engine_results(
    engine: EngineId,
    results: Vec<SearchResult>,
    limit: usize,
) -> std::result::Result<Vec<NormalizedResult>, EngineError> {
    let had_results = !results.is_empty();
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for (position, mut result) in results.into_iter().enumerate() {
        let Some(identity) = result_identity(&result.url) else {
            continue;
        };
        result.url = result.url.trim().to_owned();
        // The response is already byte bounded. Preserve returned evidence;
        // only the existing runtime output budget may truncate (and mark it).
        result.date = result.date.as_deref().and_then(|date| {
            time::OffsetDateTime::parse(date, &time::format_description::well_known::Rfc3339)
                .ok()?
                .to_offset(time::UtcOffset::UTC)
                .format(&time::format_description::well_known::Rfc3339)
                .ok()
        });
        if result.rerank_score.is_some_and(|score| !score.is_finite())
            || !(-1..=4).contains(&result.authority)
        {
            return Err(EngineError::new(engine, SearchErrorCode::ProtocolError));
        }
        if seen.insert(identity.clone()) {
            normalized.push(NormalizedResult {
                identity,
                rank: position + 1,
                result,
            });
            if normalized.len() == limit {
                break;
            }
        }
    }
    if normalized.is_empty() && had_results {
        return Err(EngineError::new(engine, SearchErrorCode::ProtocolError));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_tracking_and_fragments_but_preserves_meaningful_query_parameters() {
        assert_eq!(
            result_identity("HTTPS://Example.com/article?lang=zh&utm_source=mail&id=7#part")
                .as_deref(),
            Some("https://example.com/article?lang=zh&id=7"),
        );
    }

    #[test]
    fn rejects_non_http_urls_and_merges_only_duplicates_with_same_identity() {
        assert!(result_identity("javascript:alert(1)").is_none());
        let results = vec![
            SearchResult::text(
                "first".into(),
                "https://example.com/p?utm_campaign=a".into(),
                "one".into(),
            ),
            SearchResult::text(
                "duplicate".into(),
                "https://example.com/p".into(),
                "two".into(),
            ),
            SearchResult::text(
                "other".into(),
                "https://example.com/p?id=2".into(),
                "three".into(),
            ),
        ];
        let normalized = normalize_engine_results(EngineId::Deepseek, results, 10).unwrap();
        assert_eq!(normalized.len(), 2);
        assert_eq!(normalized[0].result.title, "first");
    }
}
