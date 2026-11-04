//! H271: calendar totality, only deterministic local calendar values.
use super::*;
use time::macros::datetime;

#[test]
fn h271_every_impossible_month_day_has_no_next_match() {
    for (month, day) in [(2, 30), (2, 31), (4, 31), (6, 31), (9, 31), (11, 31)] {
        let fields = parse_5_field(&format!("0 0 {day} {month} *")).unwrap();
        assert_eq!(next_local_match(&fields, datetime!(2026-01-01 00:00)), None);
    }
}

#[test]
fn h271_date_range_exhaustion_is_none_for_every_jump() {
    for expression in ["* * * * *", "0 0 * 1 *", "0 0 1 * *", "0 0 * * *"] {
        let fields = parse_5_field(expression).unwrap();
        assert_eq!(next_local_match(&fields, PrimitiveDateTime::MAX), None);
        assert_eq!(
            next_local_match(&fields, PrimitiveDateTime::MAX - time::Duration::hours(2)),
            if expression == "* * * * *" {
                Some(floor_minute(PrimitiveDateTime::MAX) - time::Duration::minutes(119))
            } else {
                None
            }
        );
    }
}

#[test]
fn h271_large_steps_do_not_overflow_field_expansion() {
    let fields =
        parse_5_field("*/2147483647 */2147483647 */2147483647 */2147483647 */2147483647").unwrap();
    assert_eq!(fields.minute, vec![0]);
    assert_eq!(fields.hour, vec![0]);
    assert_eq!(fields.day_of_month, vec![1]);
    assert_eq!(fields.month, vec![1]);
    assert_eq!(fields.day_of_week, vec![0]);
    assert_eq!(
        parse_5_field("0 0 1-31/2147483647 * *")
            .unwrap()
            .day_of_month,
        vec![1]
    );
}

#[test]
fn h271_century_leap_gap_and_dom_dow_or_stay_valid() {
    let leap = parse_5_field("0 0 29 2 *").unwrap();
    assert_eq!(
        next_local_match(&leap, datetime!(2096-03-01 00:00)),
        Some(datetime!(2104-02-29 00:00))
    );
    let sunday = parse_5_field("0 0 31 2 0").unwrap();
    assert_eq!(
        next_local_match(&sunday, datetime!(2026-01-01 00:00)),
        Some(datetime!(2026-02-01 00:00))
    );
}

#[test]
fn h271_public_invalid_fields_are_not_a_calendar_search() {
    let mut fields = parse_5_field("* * * * *").unwrap();
    fields.month = vec![13];
    assert!(!fields.is_valid());
    assert_eq!(next_local_match(&fields, datetime!(2026-01-01 00:00)), None);
}

#[test]
fn h271_range_start_and_duplicate_public_fields_keep_set_semantics() {
    let all = parse_5_field("* * * * *").unwrap();
    assert_eq!(
        next_local_match(&all, PrimitiveDateTime::MIN),
        PrimitiveDateTime::MIN.checked_add(time::Duration::minutes(1))
    );
    let mut fields = parse_5_field("0 0 15 * 1").unwrap();
    fields.day_of_month = vec![15; 31];
    assert_eq!(
        next_local_match(&fields, datetime!(2026-09-08 00:00)),
        Some(datetime!(2026-09-14 00:00))
    );
}

#[test]
fn h271_calendar_jumps_match_an_independent_minute_walk() {
    for expression in ["*/17 0,12 * * *", "0 0 31 * *", "0 0 31 2 7", "5 1 * * 1-5"] {
        let fields = parse_5_field(expression).unwrap();
        for from in [
            datetime!(2026-01-31 23:59:59),
            datetime!(2028-02-28 23:59:59),
        ] {
            let expected = (1..=400 * 24 * 60)
                .map(|minute| floor_minute(from) + time::Duration::minutes(minute))
                .find(|t| {
                    let dom = fields.day_of_month.contains(&(t.day() as i32));
                    let dow = fields
                        .day_of_week
                        .contains(&(t.date().weekday().number_days_from_sunday() as i32));
                    let day = if fields.day_of_month.len() == 31 {
                        dow
                    } else if fields.day_of_week.len() == 7 {
                        dom
                    } else {
                        dom || dow
                    };
                    fields.month.contains(&(t.month() as i32))
                        && day
                        && fields.hour.contains(&(t.hour() as i32))
                        && fields.minute.contains(&(t.minute() as i32))
                });
            assert!(
                expected.is_some(),
                "oracle window must contain {expression}"
            );
            assert_eq!(next_local_match(&fields, from), expected, "{expression}");
        }
    }
}

#[test]
fn h271_epoch_conversion_at_calendar_bounds_does_not_panic() {
    let offset = time::OffsetDateTime::now_local()
        .map(|local| local.offset())
        .unwrap_or(time::UtcOffset::UTC);
    for date in [PrimitiveDateTime::MIN, PrimitiveDateTime::MAX] {
        let ms = (date.assume_utc().unix_timestamp_nanos() / 1_000_000) as i64;
        let utc = time::OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000).unwrap();
        let expected = utc
            .checked_to_offset(offset)
            .map(|local| PrimitiveDateTime::new(local.date(), local.time()));
        assert_eq!(epoch_ms_to_local(ms), expected);
    }
}

#[test]
fn h271_rejected_creation_does_not_publish_or_rewrite_the_schedule() {
    use crate::schedule::ScheduledTaskStore;
    let root = tempfile::tempdir().unwrap();
    let store = ScheduledTaskStore::new(root.path());
    for before_exists in [false, true] {
        let before = std::fs::read(store.path()).ok();
        for (cron, anchor) in [
            ("0 0 31 2 *", 1_000),
            ("bad", 1_000),
            ("* * * * *", i64::MAX),
        ] {
            assert!(store
                .add(cron.into(), "H271 synthetic".into(), false, anchor)
                .is_err());
            assert_eq!(std::fs::read(store.path()).ok(), before);
        }
        if !before_exists {
            store
                .add("0 0 29 2 *".into(), "valid leap day".into(), false, 1_000)
                .unwrap();
        }
    }
}
