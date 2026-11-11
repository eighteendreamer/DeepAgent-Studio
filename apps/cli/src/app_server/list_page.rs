//! One metadata page; reuse the existing bounded continuation codec/key. No
//! server-side cursor cache, session replay, credential store or second catalog.
use super::{history_cursor, RpcResponse, ThreadListRequest};
use deepagent_core::error::{CoreError, Result};
use deepagent_harness_protocol::{DEFAULT_SESSION_PAGE_SIZE, MAX_SESSION_PAGE_SIZE};
use deepagent_persistence::bounded_read::json_len;
use deepagent_persistence::event_store::{
    EventStore, SessionCatalogFilter, SessionCatalogPosition,
};
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// Not a valid session ID: history tokens cannot cross into the catalog domain.
const SCOPE: &str = "\0thread/list.catalog.v1";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Continuation {
    version: u32,
    filters: SessionCatalogFilter,
    position: SessionCatalogPosition,
}

fn filters(request: &ThreadListRequest) -> SessionCatalogFilter {
    SessionCatalogFilter {
        archived: request.archived.unwrap_or(false),
        cwd: request.cwd.clone(),
        provider: request.provider.clone(),
        search: request.search.clone(),
        updated_after: request.updated_after,
        sort_key: request.sort_key.unwrap_or_default(),
        sort_direction: request.sort_direction.unwrap_or_default(),
    }
}
fn checked_filters(request: &ThreadListRequest, previous: &SessionCatalogFilter) -> Result<()> {
    let supplied = filters(request);
    if request.archived.is_some() && supplied.archived != previous.archived
        || request.cwd.is_some() && supplied.cwd != previous.cwd
        || request.provider.is_some() && supplied.provider != previous.provider
        || request.search.is_some() && supplied.search != previous.search
        || request.updated_after.is_some() && supplied.updated_after != previous.updated_after
        || request.sort_key.is_some() && supplied.sort_key != previous.sort_key
        || request.sort_direction.is_some() && supplied.sort_direction != previous.sort_direction
    {
        return Err(CoreError::invalid(
            "session catalog cursor belongs to different filters or ordering",
        ));
    }
    Ok(())
}
fn too_large() -> CoreError {
    CoreError::invalid("catalog_item_too_large: metadata or response envelope exceeds page budget")
}

pub(super) fn read(db: &Database, id: Value, request: ThreadListRequest) -> Result<RpcResponse> {
    let limit = request.limit.unwrap_or(DEFAULT_SESSION_PAGE_SIZE);
    let max_bytes = request.max_bytes.unwrap_or(256 * 1024);
    if !(1..=MAX_SESSION_PAGE_SIZE).contains(&limit) || !(4096..=1024 * 1024).contains(&max_bytes) {
        return Err(CoreError::invalid(
            "catalog limit must be 1..=100 and maxBytes 4096..=1048576",
        ));
    }
    let continuation: Option<Continuation> = request
        .cursor
        .as_ref()
        .map(|token| history_cursor::decode(db, SCOPE, token))
        .transpose()?;
    let filters = match &continuation {
        Some(cursor) => {
            if cursor.version != 1 {
                return Err(CoreError::invalid(
                    "unsupported session catalog cursor version",
                ));
            }
            checked_filters(&request, &cursor.filters)?;
            cursor.filters.clone()
        }
        None => filters(&request),
    };
    let page = EventStore::new(db).list_session_page(
        &filters,
        continuation.as_ref().map(|cursor| &cursor.position),
        limit,
        max_bytes,
    )?;
    let mut response = RpcResponse::success(
        id,
        json!({"threads":[],"listPageVersion":1,"hasMore":false,"nextCursor":null}),
    );
    let mut selected = None;
    let mut trimmed = false;
    for row in page.rows {
        let session = row.session;
        let item = json!({"threadId":session.id.to_string(),"title":session.title,"cwd":session.project,"createdAt":session.created_at.as_millis(),"updatedAt":session.updated_at.as_millis(),"ended":session.ended_at.is_some(),"archived":filters.archived});
        let next = Continuation {
            version: 1,
            filters: filters.clone(),
            position: row.position,
        };
        let value = response.result.as_mut().unwrap();
        value["threads"].as_array_mut().unwrap().push(item);
        value["hasMore"] = json!(true);
        value["nextCursor"] = json!("x".repeat(history_cursor::encoded_len(&next)?));
        if json_len(&response, max_bytes).is_err() {
            let items = response.result.as_mut().unwrap()["threads"]
                .as_array_mut()
                .unwrap();
            items.pop();
            if items.is_empty() {
                return Err(too_large());
            }
            trimmed = true;
            break;
        }
        selected = Some(next);
    }
    let has_more = trimmed || page.has_more;
    let next = if has_more {
        let mut next = selected.unwrap_or(Continuation {
            version: 1,
            filters,
            position: page.end.clone(),
        });
        // Include skipped provider/visibility candidates so even empty pages
        // advance, but never advance past a byte-rejected result.
        if !trimmed {
            next.position = page.end;
        }
        Some(history_cursor::encode(db, SCOPE, &next)?)
    } else {
        None
    };
    let value = response.result.as_mut().unwrap();
    value["hasMore"] = json!(has_more);
    value["nextCursor"] = json!(next);
    json_len(&response, max_bytes).map_err(|_| too_large())?;
    Ok(response)
}
