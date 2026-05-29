//! Time handling.
//!
//! The runtime records wall-clock timestamps on every event. To keep that
//! testable and deterministic, all time access goes through the [`Clock`]
//! trait rather than calling [`OffsetDateTime::now_utc`] directly.

use time::OffsetDateTime;

/// A UTC timestamp with millisecond-friendly serialization.
///
/// Stored internally as Unix milliseconds so it round-trips cleanly through
/// SQLite (`INTEGER`) and JSON without precision surprises.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    /// Construct from Unix milliseconds.
    pub const fn from_millis(ms: i64) -> Self {
        Self(ms)
    }

    /// The Unix epoch (0 ms).
    pub const EPOCH: Timestamp = Timestamp(0);

    /// Unix milliseconds since the epoch.
    pub const fn as_millis(&self) -> i64 {
        self.0
    }

    /// Convert to a [`time::OffsetDateTime`] in UTC.
    pub fn to_datetime(&self) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp_nanos((self.0 as i128) * 1_000_000)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH)
    }

    /// Duration in milliseconds between two timestamps (`self - earlier`).
    pub const fn millis_since(&self, earlier: Timestamp) -> i64 {
        self.0 - earlier.0
    }
}

impl std::fmt::Display for Timestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // RFC3339-ish for human readability; falls back to raw millis.
        match self
            .to_datetime()
            .format(&time::format_description::well_known::Rfc3339)
        {
            Ok(s) => f.write_str(&s),
            Err(_) => write!(f, "{}ms", self.0),
        }
    }
}

impl std::fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Timestamp({} | {})", self.0, self)
    }
}

impl serde::Serialize for Timestamp {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for Timestamp {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        i64::deserialize(d).map(Timestamp)
    }
}

/// Abstraction over "what time is it now", to keep the runtime testable.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Current wall-clock time in UTC.
    fn now(&self) -> Timestamp;
}

/// The real system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let now = OffsetDateTime::now_utc();
        Timestamp((now.unix_timestamp_nanos() / 1_000_000) as i64)
    }
}

/// A controllable clock for tests. Time only advances when you tell it to.
#[derive(Debug)]
pub struct FixedClock {
    millis: std::sync::atomic::AtomicI64,
}

impl FixedClock {
    /// Create a fixed clock starting at `start_ms`.
    pub fn new(start_ms: i64) -> Self {
        Self {
            millis: std::sync::atomic::AtomicI64::new(start_ms),
        }
    }

    /// Advance the clock by `delta_ms` milliseconds.
    pub fn advance(&self, delta_ms: i64) {
        self.millis
            .fetch_add(delta_ms, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        Timestamp(self.millis.load(std::sync::atomic::Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_clock_advances() {
        let c = FixedClock::new(1_000);
        assert_eq!(c.now().as_millis(), 1_000);
        c.advance(500);
        assert_eq!(c.now().as_millis(), 1_500);
    }

    #[test]
    fn system_clock_is_after_epoch() {
        let c = SystemClock;
        assert!(c.now().as_millis() > 1_700_000_000_000);
    }

    #[test]
    fn timestamp_serde_roundtrip() {
        let t = Timestamp::from_millis(1_748_492_921_000);
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, "1748492921000");
        let back: Timestamp = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn millis_since_computes_delta() {
        let a = Timestamp::from_millis(1000);
        let b = Timestamp::from_millis(2500);
        assert_eq!(b.millis_since(a), 1500);
    }
}
