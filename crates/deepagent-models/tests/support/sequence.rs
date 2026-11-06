//! Explicit producer-side numbering for old synthetic Responses fixtures.
//! Never used by a production parser/transport or raw sequence-negative tests.
//! Existing sequence fields (including null/invalid values) are never repaired.
use deepagent_core::error::Result;
use deepagent_models::transport::EventSink;
use deepagent_models::{DeltaObserver, MockTransport, Response, ResponseAccumulator};
use serde_json::Value;

#[derive(Default)]
pub struct FixtureSequence(u64);

impl FixtureSequence {
    pub fn event(&mut self, text: &str) -> String {
        let Ok(mut event) = serde_json::from_str::<Value>(text) else {
            return text.to_owned();
        };
        if !event
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.starts_with("response."))
        {
            return text.to_owned();
        }
        if event.get("sequence_number").is_none() {
            event["sequence_number"] = self.0.into();
        }
        if let Some(sequence) = event["sequence_number"].as_u64() {
            self.0 = sequence.saturating_add(1);
        }
        event.to_string()
    }
}

pub fn numbered(events: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut sequence = FixtureSequence::default();
    events
        .into_iter()
        .map(|event| sequence.event(&event))
        .collect()
}

pub fn numbered_mock(mut transport: MockTransport) -> MockTransport {
    transport.events = numbered(transport.events);
    transport
}

pub fn mock(events: impl IntoIterator<Item = String>) -> MockTransport {
    numbered_mock(MockTransport::new(events))
}

pub fn sink(sink: &mut dyn EventSink) -> impl EventSink + '_ {
    let mut sequence = FixtureSequence::default();
    move |event: &str| sink.on_event(&sequence.event(event))
}

/// An explicitly selected fixture producer in front of the real accumulator.
/// This only attaches omitted sequence metadata; all parsing and finish/error
/// behavior remains the production ResponseAccumulator's implementation.
#[derive(Default)]
pub struct NumberedAccumulator {
    inner: ResponseAccumulator,
    sequence: FixtureSequence,
}

impl NumberedAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_sse_data_observed(
        &mut self,
        data: &str,
        observer: &mut dyn DeltaObserver,
    ) -> Result<bool> {
        self.inner
            .push_sse_data_observed(&self.sequence.event(data), observer)
    }

    pub fn finish(self) -> Result<Response> {
        self.inner.finish()
    }
}
