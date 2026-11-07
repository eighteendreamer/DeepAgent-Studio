//! Actual bundled SQLite, one EventStore and synthetic authority files only.
use super::*;
use deepagent_core::{event::EventPayload, message::Message};
use std::collections::HashSet;

fn seed(db: &Database, project: Option<&str>, timestamp: i64) -> SessionId {
    let id = SessionId::new();
    let store = EventStore::new(db);
    store
        .create_session_full(
            id,
            Some("synthetic"),
            SessionMode::default(),
            project,
            Timestamp::from_millis(timestamp),
        )
        .unwrap();
    store
        .append(
            id,
            EventPayload::MessageAppended {
                message: Message::user("检 缓存 selectionneedle"),
            },
            Timestamp::from_millis(timestamp + 1),
        )
        .unwrap();
    id
}

#[test]
fn h228_sql_sets_precede_limit_and_do_not_spend_sqlite_variables_per_member() {
    let db = Database::open_in_memory().unwrap();
    let store = EventStore::new(&db);
    let live = seed(&db, None, 1);
    let hidden = seed(&db, Some("closed'[] OR 1=1"), 5000);
    let other = seed(&db, Some("open"), 6000);
    for i in 0..105 {
        store
            .append(
                live,
                EventPayload::MessageAppended {
                    message: Message::user("检 缓存 selectionneedle"),
                },
                Timestamp::from_millis(10 + i),
            )
            .unwrap();
    }
    let mut excluded: HashSet<_> = (0..1200).map(|i| format!("excluded-{i}'[]")).collect();
    excluded.insert(hidden.to_string());
    let projects = HashSet::new();
    let filter = SessionSearchFilter {
        excluded_sessions: Some(&excluded),
        allowed_projects: Some(&projects),
    };
    let control = SessionSearchControl::default();
    for query in ["检", "缓存", "selectionneedle"] {
        for limit in [0, 1, 100, usize::MAX] {
            let report = store
                .search_with_control_filtered(
                    query,
                    SessionSearchScope::All,
                    limit,
                    &control,
                    Some(filter),
                )
                .unwrap();
            assert!(report.unavailable.is_empty());
            assert_eq!(report.hits.len(), limit.clamp(1, 100));
            assert!(report.hits.iter().all(|h| h.session.id == live));
            assert!(report
                .hits
                .windows(2)
                .all(|p| p[0].sequence > p[1].sequence));
        }
        let raw = store.search(query, SessionSearchScope::All, 1).unwrap();
        assert_eq!(
            raw[0].session.id, other,
            "legacy/tool selection must not acquire UI visibility implicitly"
        );
        assert!(store
            .search_with_control_filtered(
                query,
                SessionSearchScope::Project("open"),
                1,
                &control,
                Some(filter)
            )
            .unwrap()
            .hits
            .is_empty());
    }
    let allowed: HashSet<_> = ["open".into()].into_iter().collect();
    let selected = store
        .search_with_control_filtered(
            "检",
            SessionSearchScope::Project("open"),
            1,
            &control,
            Some(SessionSearchFilter {
                allowed_projects: Some(&allowed),
                ..filter
            }),
        )
        .unwrap();
    assert_eq!(selected.hits[0].session.id, other);
}

#[test]
fn h228_eligibility_is_shared_by_health_budget_cancel_and_the_unfiltered_control() {
    let db = Database::open_in_memory().unwrap();
    let store = EventStore::new(&db);
    let live = seed(&db, None, 1);
    let hidden = seed(&db, Some("closed"), 2);
    let orphan = SessionId::new();
    store
        .mark_search_unavailable(hidden, SessionSearchIssueKind::Busy)
        .unwrap();
    store
        .mark_search_unavailable(orphan, SessionSearchIssueKind::Missing)
        .unwrap();
    let projects = HashSet::new();
    let filter = SessionSearchFilter {
        allowed_projects: Some(&projects),
        ..Default::default()
    };
    let zero = SessionSearchControl::new(std::time::Duration::ZERO, &|| false);
    let report = store
        .search_with_control_filtered("检", SessionSearchScope::All, 1, &zero, Some(filter))
        .unwrap();
    assert!(report.hits.is_empty());
    assert_eq!(report.unavailable.len(), 1);
    assert_eq!(report.unavailable[0].session_id, live);
    assert_eq!(report.unavailable[0].kind, SessionSearchIssueKind::Deferred);
    assert!(store
        .search_file_issues(SessionSearchScope::All)
        .unwrap()
        .iter()
        .any(|i| i.session_id == orphan));
    assert!(!store
        .search_file_issues(SessionSearchScope::NoProject)
        .unwrap()
        .iter()
        .any(|i| i.session_id == orphan));
    let cancelled = SessionSearchControl::new(std::time::Duration::from_secs(1), &|| true);
    assert!(store
        .search_with_control_filtered("检", SessionSearchScope::All, 1, &cancelled, Some(filter))
        .unwrap_err()
        .to_string()
        .contains("session_search_cancelled"));
    let recovered = store
        .search_with_control_filtered(
            "检",
            SessionSearchScope::All,
            1,
            &SessionSearchControl::default(),
            Some(filter),
        )
        .unwrap();
    assert_eq!(recovered.hits.len(), 1);
    assert!(recovered.unavailable.is_empty());
}

#[test]
fn h228_recycle_after_maintenance_cannot_be_reported_as_a_complete_empty_page() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("race.sqlite");
    let db = Database::open(&path).unwrap();
    let store = EventStore::new(&db);
    let older = seed(&db, None, 100);
    let newest = seed(&db, None, 200);
    // A separate real connection observes the maintenance commit without
    // re-entering Database's connection mutex from its cooperative checkpoint.
    let concurrent = Database::open(&path).unwrap();
    store
        .mark_search_unavailable(newest, SessionSearchIssueKind::Busy)
        .unwrap();
    let recycled = AtomicBool::new(false);
    let checkpoint = || {
        if !recycled.load(Ordering::SeqCst) {
            let pending: usize = concurrent.with_conn(|c| {
                c.query_row("SELECT count(*) FROM documents WHERE collection='session_file_health_v1' AND id=?1", [newest.to_string()], |r|r.get(0)).map_err(map_sqlite)
            }).unwrap();
            // Ready is an actual committed projection/health transition, not
            // a fragile callback count, sleep, or new production test hook.
            if pending == 0 && !recycled.swap(true, Ordering::SeqCst) {
                assert!(EventStore::new(&concurrent)
                    .trash_session(newest, Timestamp::from_millis(5000))
                    .unwrap());
            }
        }
        false
    };
    let control = SessionSearchControl::new(std::time::Duration::from_secs(5), &checkpoint);
    let projects = HashSet::new();
    let filter = SessionSearchFilter {
        allowed_projects: Some(&projects),
        ..Default::default()
    };
    let report = store
        .search_with_control_filtered("检", SessionSearchScope::All, 1, &control, Some(filter))
        .unwrap();
    assert!(recycled.load(Ordering::SeqCst));
    assert!(report.hits.is_empty());
    assert_eq!(
        report.unavailable.len(),
        1,
        "losing a selected source after maintenance is partial, not a complete empty result"
    );
    assert_eq!(report.unavailable[0].session_id, newest);
    assert_eq!(report.unavailable[0].kind, SessionSearchIssueKind::Deferred);
    assert!(report.require_complete().is_err());
    let next = store
        .search_with_control_filtered(
            "检",
            SessionSearchScope::All,
            1,
            &SessionSearchControl::default(),
            Some(filter),
        )
        .unwrap();
    assert!(next.unavailable.is_empty());
    assert_eq!(next.hits[0].session.id, older);
}
