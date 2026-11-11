use deepagent_harness_protocol::{decode_thread_list_page, HarnessRequest, ThreadListRequest};
use serde_json::json;

#[test]
fn h220_catalog_options_are_typed_and_legacy_empty_request_survives() {
    let request: HarnessRequest =
        serde_json::from_value(json!({"method":"thread/list","params":{}})).unwrap();
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({"method":"thread/list","params":{}})
    );
    let params = json!({"archived":true,"limit":25,"cursor":"opaque","cwd":"synthetic","provider":"deepseek","search":"_%","updatedAfter":123,"sortKey":"created_at","sortDirection":"asc","maxBytes":8192});
    let request: HarnessRequest =
        serde_json::from_value(json!({"method":"thread/list","params":params})).unwrap();
    assert_eq!(serde_json::to_value(request).unwrap()["params"], params);
    for params in [
        json!({"limit":"1"}),
        json!({"sortDirection":"invalid"}),
        json!({"updatedAfter":1.5}),
        json!({"archived":"true"}),
        json!({"unsupportedFilter":true}),
    ] {
        assert!(serde_json::from_value::<HarnessRequest>(
            json!({"method":"thread/list","params":params})
        )
        .is_err());
    }
}

#[test]
fn h220_explicit_page_reader_rejects_legacy_duplicate_oversized_and_looping_pages() {
    let request = ThreadListRequest {
        limit: Some(1),
        cursor: Some("previous".into()),
        ..Default::default()
    };
    let row = json!({"threadId":"synthetic","title":null,"cwd":null,"createdAt":1,"updatedAt":2,"ended":false,"archived":false});
    let valid = json!({"listPageVersion":1,"threads":[row],"hasMore":true,"nextCursor":"next"});
    assert!(decode_thread_list_page(&request, &valid).is_ok());
    for invalid in [
        json!({"threads":[]}),
        json!({"listPageVersion":2,"threads":[],"hasMore":false,"nextCursor":null}),
        json!({"listPageVersion":1,"threads":[row,row],"hasMore":false,"nextCursor":null}),
        json!({"listPageVersion":1,"threads":[],"hasMore":true,"nextCursor":"previous"}),
        json!({"listPageVersion":1,"threads":[],"hasMore":false,"nextCursor":"extra"}),
        json!({"listPageVersion":1,"threads":[],"hasMore":false}),
    ] {
        assert!(
            decode_thread_list_page(&request, &invalid).is_err(),
            "{invalid}"
        );
    }
}
