//! Minimal 5-field cron parsing and next-run calculation.
//!
//! Rust counterpart to Claude Code's `cron.js` (`借鉴/claudecode/restored-src/src/utils/cron.ts`):
//! the standard 5-field subset — `minute hour day-of-month month day-of-week` —
//! with wildcard, `N`, step (`*/N`), range (`N-M`), and comma-list syntax.
//! No `L` / `W` / `?` / name aliases.
//!
//! Times are interpreted in the process's *local* timezone ("0 9 * * *" means
//! 9am wherever the CLI runs), matching CC. The parse and the next-match
//! computation are pure calendar math without any timezone conversion, so they
//! are fully deterministic and unit-testable; the timezone boundary lives in
//! [`super::scheduler`] where wall-clock epochs are translated to naive local
//! calendar minutes.

use std::collections::BTreeSet;

use deepagent_core::error::{CoreError, Result};
use time::{Date, Month, PrimitiveDateTime, Time};

/// Expanded cron fields: each holds the sorted set of matching values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronFields {
    pub minute: Vec<i32>,
    pub hour: Vec<i32>,
    pub day_of_month: Vec<i32>,
    pub month: Vec<i32>,
    pub day_of_week: Vec<i32>,
}

impl CronFields {
    pub fn is_valid(&self) -> bool {
        !self.minute.is_empty()
            && !self.hour.is_empty()
            && !self.day_of_month.is_empty()
            && !self.month.is_empty()
            && !self.day_of_week.is_empty()
    }
}

/// One cron field spec: (parser, min, max, accept_7_as_sunday).
type FieldSpec = (fn(&str, i32, i32, bool) -> Option<Vec<i32>>, i32, i32, bool);

/// Parse a 5-field cron expression into expanded value sets. Returns an error
/// for anything outside the supported subset (field count, syntax, out-of-range).
pub fn parse_5_field(expr: &str) -> Result<CronFields> {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(CoreError::invalid(format!(
            "cron expression must have 5 fields, got {}: {expr:?}",
            parts.len()
        )));
    }
    // minute, hour, day-of-month, month, day-of-week
    let specs: [FieldSpec; 5] = [
        (expand_field, 0, 59, false), // minute
        (expand_field, 0, 23, false), // hour
        (expand_field, 1, 31, false), // day-of-month
        (expand_field, 1, 12, false), // month
        (expand_field, 0, 6, true),   // day-of-week (7 accepted as Sunday alias)
    ];
    let mut out = Vec::with_capacity(5);
    for (parser, min, max, dow) in &specs {
        let vals = parser(parts[out.len()], *min, *max, *dow).ok_or_else(|| {
            CoreError::invalid(format!("invalid cron field: {:?}", parts[out.len()]))
        })?;
        out.push(vals);
    }
    let fields = CronFields {
        minute: out[0].clone(),
        hour: out[1].clone(),
        day_of_month: out[2].clone(),
        month: out[3].clone(),
        day_of_week: out[4].clone(),
    };
    if !fields.is_valid() {
        return Err(CoreError::invalid("cron fields must not be empty"));
    }
    Ok(fields)
}

/// Expand a single cron field into a sorted list of matching values.
/// Supports wildcard, `N`, `N-M`, `*/step`, `N-M/step`, and comma lists.
fn expand_field(field: &str, min: i32, max: i32, dow: bool) -> Option<Vec<i32>> {
    let eff_max = if dow { max + 1 } else { max };
    let mut out = BTreeSet::new();
    for part in field.split(',') {
        let part = part.trim();
        // wildcard or */step
        if let Some(rest) = part.strip_prefix('*') {
            let step: i32 = if let Some(s) = rest.strip_prefix('/') {
                s.parse().ok()?
            } else if rest.is_empty() {
                1
            } else {
                return None;
            };
            if step < 1 {
                return None;
            }
            let mut i = min;
            while i <= eff_max {
                out.insert(normalize_dow(i, dow));
                i += step;
            }
            continue;
        }
        // range a-b or a-b/step
        if let Some(dash) = part.find('-') {
            let lo: i32 = part[..dash].parse().ok()?;
            let rest = &part[dash + 1..];
            let (hi, step): (i32, i32) = match rest.split_once('/') {
                Some((h, s)) => (h.parse().ok()?, s.parse().ok()?),
                None => (rest.parse().ok()?, 1),
            };
            if lo > hi || step < 1 || lo < min || hi > eff_max {
                return None;
            }
            let mut i = lo;
            while i <= hi {
                out.insert(normalize_dow(i, dow));
                i += step;
            }
            continue;
        }
        // plain N
        let n: i32 = part.parse().ok()?;
        if n < min || n > eff_max {
            return None;
        }
        out.insert(normalize_dow(n, dow));
    }
    if out.is_empty() {
        None
    } else {
        Some(out.into_iter().collect())
    }
}

/// day-of-week field: 7 is accepted as the Sunday alias and normalized to 0
/// ([`expand_field`] uses an extended effective max so `5-7` covers Fri/Sat/Sun).
fn normalize_dow(n: i32, dow: bool) -> i32 {
    if dow && n == 7 {
        0
    } else {
        n
    }
}

/// Compute the next calendar minute strictly after `from` that matches the
/// fields, walking forward minute-by-minute. Bounded at 366 days; `None` when
/// no match (impossible for valid input, but satisfies the type).
///
/// Standard cron semantics: when both day-of-month and day-of-week are
/// constrained, a date matches if **either** matches (vixie-cron OR semantics,
/// mirroring [`cron.ts`]'s `computeNextCronRun`).
pub fn next_local_match(fields: &CronFields, from: PrimitiveDateTime) -> Option<PrimitiveDateTime> {
    let minute_set: BTreeSet<i32> = fields.minute.iter().copied().collect();
    let hour_set: BTreeSet<i32> = fields.hour.iter().copied().collect();
    let dom_set: BTreeSet<i32> = fields.day_of_month.iter().copied().collect();
    let month_set: BTreeSet<i32> = fields.month.iter().copied().collect();
    let dow_set: BTreeSet<i32> = fields.day_of_week.iter().copied().collect();

    let dom_wild = fields.day_of_month.len() == 31;
    let dow_wild = fields.day_of_week.len() == 7;

    // Round up to the next whole minute, strictly after `from`.
    let mut t = floor_minute(from) + time::Duration::minutes(1);

    let max_iter = 366 * 24 * 60;
    for _ in 0..max_iter {
        let month = t.month() as u8 as i32;
        if !month_set.contains(&month) {
            // Jump to start of next month.
            t = first_of_next_month(t);
            continue;
        }

        let dom = t.day() as i32;
        let dow = t.date().weekday().number_days_from_sunday() as i32;
        let day_matches = match (dom_wild, dow_wild) {
            (true, true) => true,
            (true, false) => dow_set.contains(&dow),
            (false, true) => dom_set.contains(&dom),
            (false, false) => dom_set.contains(&dom) || dow_set.contains(&dow),
        };
        if !day_matches {
            // Jump to start of next day.
            t = start_of_next_day(t);
            continue;
        }

        let hour = t.hour() as i32;
        if !hour_set.contains(&hour) {
            // Jump to the next hour (minute field then re-tested at :00).
            t = start_of_next_hour(t);
            continue;
        }

        let minute = t.minute() as i32;
        if !minute_set.contains(&minute) {
            t += time::Duration::minutes(1);
            continue;
        }

        return Some(t);
    }
    None
}

/// The local calendar minute at `ms` (epoch milliseconds), translated through
/// `now_local`'s offset so a monotonic wall clock and the calendar view stay
/// in lockstep. Falls back to UTC when the local offset is unavailable.
pub fn epoch_ms_to_local(ms: i64) -> Option<PrimitiveDateTime> {
    let utc = time::OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000).ok()?;
    let offset = time::OffsetDateTime::now_local()
        .map(|local| local.offset())
        .unwrap_or(time::UtcOffset::UTC);
    let local = utc.to_offset(offset);
    Some(PrimitiveDateTime::new(local.date(), local.time()))
}

/// Local calendar minute back to epoch milliseconds (inverse of
/// [`epoch_ms_to_local`]).
pub fn local_to_epoch_ms(pdt: PrimitiveDateTime) -> i64 {
    let offset = time::OffsetDateTime::now_local()
        .map(|local| local.offset())
        .unwrap_or(time::UtcOffset::UTC);
    let epoch_ms = pdt.assume_offset(offset).unix_timestamp_nanos() / 1_000_000;
    epoch_ms as i64
}

fn floor_minute(t: PrimitiveDateTime) -> PrimitiveDateTime {
    PrimitiveDateTime::new(
        t.date(),
        Time::from_hms(t.hour(), t.minute(), 0).expect("h/m valid"),
    )
}

fn first_of_next_month(t: PrimitiveDateTime) -> PrimitiveDateTime {
    let (y, m) = match t.month() {
        Month::December => (t.year() + 1, Month::January),
        m => (t.year(), m.next()),
    };
    let date = Date::from_calendar_date(y, m, 1).expect("first-of-month valid");
    PrimitiveDateTime::new(date, Time::MIDNIGHT)
}

fn start_of_next_day(t: PrimitiveDateTime) -> PrimitiveDateTime {
    let next = t.date().next_day().expect("next day within date range");
    PrimitiveDateTime::new(next, Time::MIDNIGHT)
}

fn start_of_next_hour(t: PrimitiveDateTime) -> PrimitiveDateTime {
    let next = t + time::Duration::hours(1);
    PrimitiveDateTime::new(
        next.date(),
        Time::from_hms(next.hour(), 0, 0).expect("hour valid"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn parses_all_supported_syntax() {
        let f = parse_5_field("*/5 9-17 1,15 */2 1-5").unwrap();
        assert_eq!(
            f.minute.to_vec(),
            vec![0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55]
        );
        assert_eq!(f.hour, (9..=17).collect::<Vec<_>>());
        assert_eq!(f.day_of_month, vec![1, 15]);
        assert_eq!(f.month, vec![1, 3, 5, 7, 9, 11]);
        assert_eq!(f.day_of_week, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn rejects_invalid_forms() {
        assert!(parse_5_field("").is_err());
        assert!(parse_5_field("* * * *").is_err()); // 4 fields
        assert!(parse_5_field("60 * * * *").is_err()); // minute out of range
        assert!(parse_5_field("* 24 * * *").is_err()); // hour out of range
        assert!(parse_5_field("*/0 * * * *").is_err()); // zero step
        assert!(parse_5_field("a b c d e").is_err());
        assert!(parse_5_field("5-2 * * * *").is_err()); // inverted range
    }

    #[test]
    fn dow_accepts_sunday_alias() {
        let f = parse_5_field("* * * * 7").unwrap();
        assert_eq!(f.day_of_week, vec![0]); // 7 → Sunday=0
        let r = parse_5_field("* * * * 5-7").unwrap();
        // 7→0 normalization, then numeric sort (CC sorts its expanded sets).
        assert_eq!(r.day_of_week, vec![0, 5, 6]);
    }

    #[test]
    fn next_is_strictly_after_from() {
        let f = parse_5_field("30 9 * * *").unwrap();
        // 09:29:xx → 09:30 same day
        let from = datetime!(2026-09-29 09:29:45);
        assert_eq!(
            next_local_match(&f, from).unwrap(),
            datetime!(2026-09-29 09:30:00)
        );
        // exactly 09:30:00 → next day (strictly after)
        let from = datetime!(2026-09-29 09:30:00);
        assert_eq!(
            next_local_match(&f, from).unwrap(),
            datetime!(2026-09-30 09:30:00)
        );
    }

    #[test]
    fn steps_and_lists_match_across_day_boundary() {
        let f = parse_5_field("*/30 * * * *").unwrap();
        let from = datetime!(2026-09-29 23:50:00);
        assert_eq!(
            next_local_match(&f, from).unwrap(),
            datetime!(2026-09-30 00:00:00)
        );
    }

    #[test]
    fn dom_and_dow_use_or_semantics() {
        // "0 0 15 * 1" — the 15th of the month OR any Monday.
        let f = parse_5_field("0 0 15 * 1").unwrap();
        // A non-15 non-Monday lands on the next Monday.
        let from = datetime!(2026-09-08 00:00:00); // Tuesday, not the 15th
        let next = next_local_match(&f, from).unwrap();
        assert_eq!(next, datetime!(2026-09-14 00:00:00)); // next Monday
        let from = datetime!(2026-09-14 00:00:05);
        let next = next_local_match(&f, from).unwrap();
        assert_eq!(next, datetime!(2026-09-15 00:00:00)); // the 15th dominates
    }

    #[test]
    fn month_end_and_leap_year_wrap() {
        let f = parse_5_field("0 0 31 * *").unwrap();
        // Only months with 31 days can match; Feb is skipped entirely.
        let from = datetime!(2026-01-31 00:00:30);
        assert_eq!(
            next_local_match(&f, from).unwrap(),
            datetime!(2026-03-31 00:00:00)
        );

        let leap = parse_5_field("0 0 29 2 *").unwrap();
        let from = datetime!(2025-01-01 00:00:00); // 2025 is not a leap year
        assert_eq!(
            next_local_match(&leap, from).unwrap(),
            datetime!(2028-02-29 00:00:00)
        );
    }

    #[test]
    fn daily_every_hour_and_every_minute() {
        let once = parse_5_field("17 8 * * *").unwrap();
        let from = datetime!(2026-09-29 08:16:00);
        assert_eq!(
            next_local_match(&once, from).unwrap(),
            datetime!(2026-09-29 08:17:00)
        );

        let every_hour = parse_5_field("0 * * * *").unwrap();
        let from = datetime!(2026-09-29 10:00:00);
        assert_eq!(
            next_local_match(&every_hour, from).unwrap(),
            datetime!(2026-09-29 11:00:00)
        );

        let every_min = parse_5_field("* * * * *").unwrap();
        let from = datetime!(2026-09-29 10:00:00);
        assert_eq!(
            next_local_match(&every_min, from).unwrap(),
            datetime!(2026-09-29 10:01:00)
        );
    }
}
