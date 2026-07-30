//! W3C Trace Context (§7.2 observability foundation).
//!
//! # Source alignment
//!
//! - **W3C Trace Context** (`https://www.w3.org/TR/trace-context/`): the
//!   `traceparent` header wire format is `version "-" trace-id "-" parent-id
//!   "-" trace-flags`, where `version=00`, `trace-id` is 16 bytes / 32 lower-hex
//!   (not all-zero), `parent-id` (a.k.a. span-id) is 8 bytes / 16 lower-hex
//!   (not all-zero), and `trace-flags` is 1 byte / 2 hex (bit 0 = sampled).
//! - **Codex** (`codex-rs/otel/src/trace_context.rs`): propagates a
//!   `W3cTraceContext { traceparent, tracestate }` carrier and treats W3C
//!   trace-id as the correlation key across the session — the doc calls this
//!   Codex's strongest observability borrow. This module reproduces the same
//!   carrier + wire format.
//!
//! # Documented divergence (dependency-driven)
//!
//! Codex builds on the `opentelemetry` / `opentelemetry_sdk` crates
//! (`TraceContextPropagator`, tracing-opentelemetry span extensions). Those
//! crates are **not available in this offline build** (absent from
//! `Cargo.lock` and the vendored registry), so per the source-priority rule
//! ("use the crate the reference project uses — unless it is unavailable, then
//! fall back to the official spec"), this module implements the W3C wire
//! format directly with zero new dependencies. Random ids come from `uuid` v4
//! (already a workspace dependency; a v4 UUID is 16 CSPRNG bytes — exactly a
//! trace-id, and its high 8 bytes seed a span-id). An OTLP exporter can later
//! consume [`TraceContext::traceparent`] without changing this module.

use uuid::Uuid;

/// The only W3C `traceparent` version this system emits/accepts.
pub const TRACEPARENT_VERSION: &str = "00";

/// A 16-byte W3C trace id (32 lowercase hex chars, never all-zero).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TraceId([u8; 16]);

impl TraceId {
    /// Generate a random, valid trace id (CSPRNG via UUID v4).
    pub fn random() -> Self {
        // A v4 UUID is 122 random bits over 16 bytes — never all-zero in
        // practice (the version/variant nibbles are fixed non-zero), so it is
        // always a valid W3C trace-id.
        Self(*Uuid::new_v4().as_bytes())
    }

    /// Lowercase 32-hex rendering.
    pub fn to_hex(self) -> String {
        hex32(&self.0)
    }

    /// Parse from 32 lowercase-hex chars; rejects wrong length, non-hex, and
    /// the all-zero id (invalid per W3C).
    pub fn parse_hex(s: &str) -> Option<Self> {
        let bytes: [u8; 16] = parse_hex_bytes(s)?;
        if bytes.iter().all(|b| *b == 0) {
            return None;
        }
        Some(Self(bytes))
    }
}

/// An 8-byte W3C span id (16 lowercase hex chars, never all-zero).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanId([u8; 8]);

impl SpanId {
    /// Generate a random, valid span id (high 8 bytes of a fresh v4 UUID).
    pub fn random() -> Self {
        let bytes = *Uuid::new_v4().as_bytes();
        let mut id = [0u8; 8];
        id.copy_from_slice(&bytes[..8]);
        // The v4 version nibble lives in byte 6, so the high 8 bytes are never
        // all-zero; guard anyway to honor the W3C non-zero invariant.
        if id.iter().all(|b| *b == 0) {
            id[7] = 1;
        }
        Self(id)
    }

    /// Lowercase 16-hex rendering.
    pub fn to_hex(self) -> String {
        hex16(&self.0)
    }

    /// Parse from 16 lowercase-hex chars; rejects wrong length, non-hex, and
    /// the all-zero id.
    pub fn parse_hex(s: &str) -> Option<Self> {
        let bytes: [u8; 8] = parse_hex_bytes(s)?;
        if bytes.iter().all(|b| *b == 0) {
            return None;
        }
        Some(Self(bytes))
    }
}

/// A W3C span reference: `(trace_id, span_id, sampled)`. Renders to / parses
/// from a `traceparent` header value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceContext {
    /// The trace this span belongs to (stable across the whole run/trace).
    pub trace_id: TraceId,
    /// This span's id.
    pub span_id: SpanId,
    /// W3C sampled flag (trace-flags bit 0).
    pub sampled: bool,
}

impl TraceContext {
    /// A fresh root context: new random trace + span, sampled on.
    pub fn new_root() -> Self {
        Self {
            trace_id: TraceId::random(),
            span_id: SpanId::random(),
            sampled: true,
        }
    }

    /// A child span in the SAME trace: keeps `trace_id`/`sampled`, new span id.
    /// This is how the trace id "runs through" nested operations.
    pub fn child(&self) -> Self {
        Self {
            trace_id: self.trace_id,
            span_id: SpanId::random(),
            sampled: self.sampled,
        }
    }

    /// Render the W3C `traceparent` header value:
    /// `00-<trace-id>-<span-id>-<flags>`.
    pub fn traceparent(&self) -> String {
        let flags = if self.sampled { "01" } else { "00" };
        format!(
            "{TRACEPARENT_VERSION}-{}-{}-{flags}",
            self.trace_id.to_hex(),
            self.span_id.to_hex()
        )
    }

    /// Parse a W3C `traceparent` header value. Enforces the `00` version, the
    /// 4-field shape, hex field lengths, and non-zero ids. Unknown flag bits
    /// are ignored except bit 0 (sampled), per the spec's forward-compat rule.
    pub fn parse_traceparent(value: &str) -> Option<Self> {
        let mut parts = value.trim().split('-');
        let version = parts.next()?;
        let trace = parts.next()?;
        let span = parts.next()?;
        let flags = parts.next()?;
        if parts.next().is_some() {
            return None; // exactly 4 fields for version 00
        }
        if version != TRACEPARENT_VERSION {
            return None;
        }
        if flags.len() != 2 || !flags.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let flag_byte = u8::from_str_radix(flags, 16).ok()?;
        Some(Self {
            trace_id: TraceId::parse_hex(trace)?,
            span_id: SpanId::parse_hex(span)?,
            sampled: flag_byte & 0x01 == 0x01,
        })
    }
}

/// W3C propagation carrier (aligned with Codex `W3cTraceContext`): the pair of
/// header values passed between processes. `tracestate` is vendor key/values;
/// this system does not yet populate it but round-trips it for propagation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct W3cTraceContext {
    /// The `traceparent` header value, when a valid span context exists.
    pub traceparent: Option<String>,
    /// The `tracestate` header value, when present.
    pub tracestate: Option<String>,
}

impl W3cTraceContext {
    /// Build a carrier from a span context (no tracestate).
    pub fn from_context(ctx: &TraceContext) -> Self {
        Self {
            traceparent: Some(ctx.traceparent()),
            tracestate: None,
        }
    }

    /// Recover the span context from the carrier's `traceparent`, if valid.
    pub fn context(&self) -> Option<TraceContext> {
        self.traceparent
            .as_deref()
            .and_then(TraceContext::parse_traceparent)
    }
}

fn hex32(bytes: &[u8; 16]) -> String {
    let mut s = String::with_capacity(32);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex16(bytes: &[u8; 8]) -> String {
    let mut s = String::with_capacity(16);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Parse exactly `N` bytes from `2*N` lowercase-hex chars. Rejects wrong
/// length, uppercase (W3C mandates lowercase), and non-hex.
fn parse_hex_bytes<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    // W3C requires lowercase hex; reject anything else to stay strict.
    if s.bytes().any(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let mut out = [0u8; N];
    let bytes = s.as_bytes();
    for (i, slot) in out.iter_mut().enumerate() {
        let hi = (bytes[i * 2] as char).to_digit(16)?;
        let lo = (bytes[i * 2 + 1] as char).to_digit(16)?;
        *slot = (hi * 16 + lo) as u8;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traceparent_round_trips() {
        let ctx = TraceContext::new_root();
        let tp = ctx.traceparent();
        // Shape: 00-<32hex>-<16hex>-01
        let parts: Vec<&str> = tp.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], "00");
        assert_eq!(parts[1].len(), 32);
        assert_eq!(parts[2].len(), 16);
        assert_eq!(parts[3], "01");

        let parsed = TraceContext::parse_traceparent(&tp).expect("valid traceparent");
        assert_eq!(parsed, ctx);
    }

    #[test]
    fn child_keeps_trace_id_and_flags_but_new_span() {
        let root = TraceContext::new_root();
        let child = root.child();
        assert_eq!(child.trace_id, root.trace_id, "trace id runs through");
        assert_eq!(child.sampled, root.sampled);
        assert_ne!(child.span_id, root.span_id, "child gets a fresh span id");
    }

    #[test]
    fn parses_a_known_good_traceparent() {
        // From the W3C spec examples.
        let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let ctx = TraceContext::parse_traceparent(tp).expect("spec example must parse");
        assert_eq!(ctx.trace_id.to_hex(), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(ctx.span_id.to_hex(), "00f067aa0ba902b7");
        assert!(ctx.sampled);
        // Re-render is byte-identical.
        assert_eq!(ctx.traceparent(), tp);
    }

    #[test]
    fn rejects_malformed_traceparents() {
        // All-zero trace id / span id are invalid per W3C.
        assert!(TraceContext::parse_traceparent(
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01"
        )
        .is_none());
        assert!(TraceContext::parse_traceparent(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01"
        )
        .is_none());
        // Wrong version.
        assert!(TraceContext::parse_traceparent(
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        )
        .is_none());
        // Wrong field count.
        assert!(TraceContext::parse_traceparent(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7"
        )
        .is_none());
        // Uppercase hex (spec mandates lowercase).
        assert!(TraceContext::parse_traceparent(
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01"
        )
        .is_none());
        // Bad length.
        assert!(TraceContext::parse_traceparent("00-abc-def-01").is_none());
        // Empty / junk.
        assert!(TraceContext::parse_traceparent("").is_none());
        assert!(TraceContext::parse_traceparent("not-a-traceparent").is_none());
    }

    #[test]
    fn unsampled_flag_round_trips() {
        let ctx = TraceContext {
            trace_id: TraceId::random(),
            span_id: SpanId::random(),
            sampled: false,
        };
        assert!(ctx.traceparent().ends_with("-00"));
        let parsed = TraceContext::parse_traceparent(&ctx.traceparent()).unwrap();
        assert!(!parsed.sampled);
    }

    #[test]
    fn carrier_round_trips_through_context() {
        let ctx = TraceContext::new_root();
        let carrier = W3cTraceContext::from_context(&ctx);
        assert!(carrier.traceparent.is_some());
        assert_eq!(carrier.context(), Some(ctx));
        // An empty carrier yields no context.
        assert_eq!(W3cTraceContext::default().context(), None);
    }

    #[test]
    fn random_ids_are_unique_and_valid() {
        let a = TraceContext::new_root();
        let b = TraceContext::new_root();
        assert_ne!(a.trace_id, b.trace_id);
        // Every generated id parses back cleanly (non-zero, right length).
        assert!(TraceId::parse_hex(&a.trace_id.to_hex()).is_some());
        assert!(SpanId::parse_hex(&a.span_id.to_hex()).is_some());
    }
}
