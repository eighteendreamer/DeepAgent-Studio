use std::time::Duration;

use super::super::dispatcher::{ReqwestSearchEngineClient, SearchEngineClient};
use super::super::types::{EngineId, SearchRequest, SearchRuntimeConfig, SearchSettings};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

async fn serve_once(status: u16, content_type: &str, body: &str) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = format!("http://{address}");
    let content_type = content_type.to_string();
    let body = body.to_string();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut chunk = [0u8; 2048];
        let expected_len = loop {
            let read = stream.read(&mut chunk).await.unwrap();
            assert_ne!(read, 0, "client closed before sending HTTP headers");
            request.extend_from_slice(&chunk[..read]);
            if let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or_default();
                let expected = header_end + 4 + content_length;
                if request.len() >= expected {
                    break expected;
                }
            }
        };
        assert!(request.len() >= expected_len);
        let reason = match status {
            200 => "OK",
            403 => "Forbidden",
            _ => "Fixture",
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8_lossy(&request[..expected_len]).into_owned()
    });
    (endpoint, task)
}

fn runtime_config(id: EngineId, base_url: Option<String>) -> SearchRuntimeConfig {
    let mut settings = SearchSettings::default();
    for engine in &mut settings.engines {
        engine.enabled = engine.id == id;
        if engine.id == id {
            engine.base_url = base_url.clone();
        }
    }
    SearchRuntimeConfig {
        settings,
        deepseek_api_key: Some("synthetic-deepseek-key".into()),
        anysearch_api_key: Some("synthetic-anysearch-key".into()),
    }
}

fn request(value: Value) -> SearchRequest {
    SearchRequest::parse(value).unwrap()
}

async fn search(
    client: &ReqwestSearchEngineClient,
    id: EngineId,
    request: &SearchRequest,
    config: &SearchRuntimeConfig,
) -> Result<Vec<super::super::types::SearchResult>, super::super::types::EngineError> {
    client
        .search(id, request, config, Duration::from_secs(2))
        .await
}

fn request_body(raw: &str) -> &str {
    raw.split_once("\r\n\r\n").unwrap().1
}

#[tokio::test]
async fn deepseek_http_mapping_uses_search_endpoint_bearer_and_no_model_field() {
    let (endpoint, server) = serve_once(
        200,
        "application/json",
        r#"{"results":[{"rerank_score":0.8,"title":"DS title","url":"https://example.test/ds","snippet":"excerpt","summary":"summary","content":"body","site_name":"Example","site_icon":"https://example.test/icon","date":"2026-11-09T00:00:00Z","authority":2}]}"#,
    )
    .await;
    let client = ReqwestSearchEngineClient::with_test_endpoints(
        format!("{endpoint}/v1/tools/search"),
        "http://127.0.0.1:1/unused".into(),
    );
    let request = request(json!({
        "query":"synthetic query", "count":3, "ori_question":"original question",
        "localization":"127.0.0.1", "need_content":false
    }));
    let config = runtime_config(EngineId::Deepseek, None);
    let rows = search(&client, EngineId::Deepseek, &request, &config)
        .await
        .unwrap();
    let raw = server.await.unwrap();
    assert!(raw.starts_with("POST /v1/tools/search HTTP/1.1\r\n"));
    assert!(raw
        .to_ascii_lowercase()
        .contains("authorization: bearer synthetic-deepseek-key\r\n"));
    let body: Value = serde_json::from_str(request_body(&raw)).unwrap();
    assert_eq!(body["query"], "synthetic query");
    assert_eq!(body["count"], 3);
    assert_eq!(body["ori_question"], "original question");
    assert_eq!(body["localization"], "127.0.0.1");
    assert_eq!(body["need_content"], false);
    assert!(body.get("model").is_none());
    assert_eq!(rows[0].title, "DS title");
    assert_eq!(rows[0].rerank_score, Some(0.8));
    assert_eq!(rows[0].authority, 2);
    assert_eq!(rows[0].content, "");
}

#[tokio::test]
async fn anysearch_http_mapping_caps_results_and_keeps_credentials_out_of_body() {
    let (base_url, server) = serve_once(
        200,
        "application/json",
        r#"{"code":0,"message":"ok","request_id":"synthetic-id","data":{"results":[{"title":"AS title","url":"https://example.test/as","snippet":"excerpt","content":"markdown body"}],"metadata":{"total_results":1,"search_time_ms":312}}}"#,
    )
    .await;
    let client = ReqwestSearchEngineClient::new();
    let request = request(json!({"query":"synthetic query", "count":20, "need_content":true}));
    let config = runtime_config(EngineId::Anysearch, Some(base_url));
    let rows = search(&client, EngineId::Anysearch, &request, &config)
        .await
        .unwrap();
    let raw = server.await.unwrap();
    assert!(raw.starts_with("POST /v1/search HTTP/1.1\r\n"));
    assert!(raw
        .to_ascii_lowercase()
        .contains("authorization: bearer synthetic-anysearch-key\r\n"));
    let body: Value = serde_json::from_str(request_body(&raw)).unwrap();
    assert_eq!(body["query"], "synthetic query");
    assert_eq!(body["max_results"], 10);
    assert_eq!(body["format"], "markdown");
    assert!(body.get("ori_question").is_none());
    assert!(body.get("localization").is_none());
    assert!(!request_body(&raw).contains("synthetic-anysearch-key"));
    assert_eq!(rows[0].content, "markdown body");
    assert_eq!(rows[0].rerank_score, None);
    assert_eq!(rows[0].authority, -1);
}

#[tokio::test]
async fn searxng_http_mapping_uses_q_and_json_without_fabricated_options() {
    let (base_url, server) = serve_once(
        200,
        "application/json",
        r#"{"query":"synthetic query","number_of_results":1,"results":[{"title":"SX title","url":"https://example.test/sx","content":"search excerpt","publishedDate":"2026-11-09T00:00:00Z","thumbnail":"https://example.test/thumbnail","score":2,"engines":["fixture"]}],"suggestions":[],"unresponsive_engines":[]}"#,
    )
    .await;
    let client = ReqwestSearchEngineClient::new();
    let request = request(json!({"query":"synthetic query", "count":20}));
    let config = runtime_config(EngineId::Searxng, Some(base_url));
    let rows = search(&client, EngineId::Searxng, &request, &config)
        .await
        .unwrap();
    let raw = server.await.unwrap();
    assert!(
        raw.starts_with("GET /search?")
            && raw.contains("format=json")
            && raw.contains("q=synthetic+query")
    );
    assert_eq!(rows[0].snippet, "search excerpt");
    assert_eq!(rows[0].content, "");
    assert_eq!(rows[0].rerank_score, None);
    assert_eq!(rows[0].authority, -1);
    assert_eq!(
        rows[0].site_icon, None,
        "a result thumbnail is not a site icon"
    );
}

#[tokio::test]
async fn duckduckgo_http_mapping_parses_html_result_and_distinguishes_non_result_pages() {
    let html = r#"<div class="result"><a class="result__a" href="//example.test/page">A &amp; B</a><a class="result__snippet">A short excerpt</a></div>"#;
    let (endpoint, server) = serve_once(200, "text/html", html).await;
    let client = ReqwestSearchEngineClient::with_test_endpoints(
        "http://127.0.0.1:1/unused".into(),
        format!("{endpoint}/html/"),
    );
    let request = request(json!({"query":"synthetic query", "count":1}));
    let config = runtime_config(EngineId::Duckduckgo, None);
    let rows = search(&client, EngineId::Duckduckgo, &request, &config)
        .await
        .unwrap();
    let raw = server.await.unwrap();
    assert!(raw.starts_with("POST /html/ HTTP/1.1\r\n"));
    assert!(raw
        .to_ascii_lowercase()
        .contains("content-type: application/x-www-form-urlencoded"));
    assert_eq!(request_body(&raw), "q=synthetic+query");
    assert_eq!(rows[0].title, "A & B");
    assert_eq!(rows[0].url, "https://example.test/page");
    assert_eq!(rows[0].snippet, "A short excerpt");
    assert_eq!(rows[0].content, "");

    for (body, expected) in [
        ("<html>no results found</html>", None),
        (
            "<html><form class=\"challenge-form\"></form></html>",
            Some("challenge"),
        ),
        (
            "<html><body>unexpected structure</body></html>",
            Some("markup_changed"),
        ),
    ] {
        let (endpoint, server) = serve_once(200, "text/html", body).await;
        let client = ReqwestSearchEngineClient::with_test_endpoints(
            "http://127.0.0.1:1/unused".into(),
            format!("{endpoint}/html/"),
        );
        let result = search(&client, EngineId::Duckduckgo, &request, &config).await;
        let _ = server.await.unwrap();
        match expected {
            None => assert!(result.unwrap().is_empty()),
            Some("challenge") => assert_eq!(
                result.unwrap_err().code,
                super::super::types::SearchErrorCode::Challenge
            ),
            Some(_) => assert_eq!(
                result.unwrap_err().code,
                super::super::types::SearchErrorCode::MarkupChanged
            ),
        }
    }
}

#[tokio::test]
async fn provider_failures_are_classified_without_returning_error_bodies() {
    let (base_url, server) = serve_once(
        200,
        "application/json",
        r#"{"code":401,"message":"synthetic-secret-must-not-leak","request_id":"synthetic-id"}"#,
    )
    .await;
    let client = ReqwestSearchEngineClient::new();
    let request = request(json!({"query":"synthetic query"}));
    let config = runtime_config(EngineId::Anysearch, Some(base_url));
    let error = search(&client, EngineId::Anysearch, &request, &config)
        .await
        .unwrap_err();
    let _ = server.await.unwrap();
    assert_eq!(
        error.code,
        super::super::types::SearchErrorCode::BusinessError
    );
    assert!(!format!("{error:?}").contains("synthetic-secret"));

    let (base_url, server) = serve_once(403, "text/plain", "JSON output is disabled").await;
    let client = ReqwestSearchEngineClient::new();
    let config = runtime_config(EngineId::Searxng, Some(base_url));
    let error = search(&client, EngineId::Searxng, &request, &config)
        .await
        .unwrap_err();
    let _ = server.await.unwrap();
    assert_eq!(error.code, super::super::types::SearchErrorCode::HttpError);
    assert_eq!(error.http_status, Some(403));
}

#[tokio::test]
async fn nondefault_advanced_parameters_are_rejected_before_network_for_other_engines() {
    let client = ReqwestSearchEngineClient::new();
    for id in [EngineId::Anysearch, EngineId::Searxng, EngineId::Duckduckgo] {
        let config = runtime_config(id, Some("http://127.0.0.1:1/unused".into()));
        for value in [
            json!({"query":"synthetic", "ori_question":"original"}),
            json!({"query":"synthetic", "localization":"127.0.0.1"}),
        ] {
            let error = search(&client, id, &request(value), &config)
                .await
                .unwrap_err();
            assert_eq!(
                error.code,
                super::super::types::SearchErrorCode::UnsupportedParameter,
                "{id:?}"
            );
        }
    }
}
