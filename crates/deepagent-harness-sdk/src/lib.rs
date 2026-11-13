//! # deepagent-harness-sdk
//!
//! A thin Rust client for the DeepAgent Harness stdio protocol.
//!
//! The SDK spawns a `deepagent-cli server --transport stdio` process, sends
//! JSON-RPC 2.0 requests over its stdin, and consumes the streamed harness
//! events (JSON-RPC notifications, method `harness/event`) from its stdout.
//! All request/response/event DTOs come from [`deepagent_harness_protocol`];
//! this crate only adds the transport and the client ergonomics, so it stays a
//! single implementation of each protocol capability.
//!
//! Live `subscribe()` values are [`HarnessNotification`]: use `.event` for the
//! existing business enum and `.event_sequence`/`.sequence_status` for wire
//! progression. Request-owned [`RequestEvents::recv_notification`] exposes the
//! same metadata; its original `recv()` remains a business-payload projection.
//! Wire gaps/reordering or bounded-bus lag require explicit durable reconciliation,
//! not guessing an ACK from counts or replaying a mutating request. No auto-ACK
//! occurs. A reconnect starts a new sequence domain; `thread/read` uses separate
//! durable cursors. Old event JSON is readable with unavailable wire metadata.
//! The reader, not retained client handles, owns the live producer. EOF, stdout
//! read failure, explicit client close or a retired partial request frame closes
//! raw subscriptions after their already queued notifications are drained.
//! Late subscriptions are immediately closed and never replay the old queue.
//! `RecvError::Closed` is a transport fact, not proof of run completion/failure;
//! reconnect explicitly and reconcile durable state before retrying mutations.
//!
//! Corrupt JSON or an invalid JSON-RPC 2.0 response/notification envelope retires
//! this same connection immediately. Pending/late calls and owned event streams
//! receive `SdkError::Decode` with the same sanitized first cause; already queued
//! events remain drainable. No peer line or serde payload text is retained in
//! that diagnostic. This is not proof that a sent mutation failed remotely.
//! Wrong/missing envelope versions keep the existing rejection-only behavior;
//! valid unknown notification methods remain ignored. No automatic replay,
//! respawn or remote cancellation is added. Public raw `decode_line` is unchanged.
//!
//! Archive uses the existing shared AppCore activity/control gate: busy owners
//! are refused, not cancelled. `thread_list()` selects active identities;
//! `thread_list_with_options(ThreadListRequest { archived: Some(true), ..Default::default() })` selects
//! only archived ones. Explicit views require matching archive projection flags.
//! `thread_unarchive` restores visibility/admission eligibility without loading
//! or replaying a thread; call `thread_resume` afterwards. Archived identities
//! cannot start/resume/steer/fork even through a stale host, while `thread_read`
//! stays read-only and available. No new event store or migration is involved.
//! `HarnessRequest` gains an additive v1 unarchive variant; `ThreadListRequest`
//! literals need `archived` (or `Default`) and exhaustive matches need updating.
//!
//! Catalog lists are now one bounded page (default 25, max 100), never an
//! automatic full download. The typed `thread_list_page` validates v1 paging;
//! raw list responses keep `hasMore`/`nextCursor`. Filters/order are scoped to
//! the opaque continuation; catalog changes require explicit first-page refresh.
//! Empty provider-filter pages may still have a continuation. SQLite V23 adds
//! a change epoch/indexes in the existing database, not a second session store.
//!
//! ## Example
//!
//! ```no_run
//! use deepagent_harness_protocol::{ThreadStartRequest, TurnStartRequest};
//! use deepagent_harness_sdk::HarnessProcess;
//! # fn main() {
//! # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
//! let bin = std::path::Path::new("target/debug/deepagent");
//! let ws = std::path::Path::new(".");
//! let process = HarnessProcess::spawn(bin, ws).await.unwrap();
//! let client = process.client();
//! client.initialize("my-app", "0.1.0").await.unwrap();
//! let mut thread = client
//!     .thread_start_streamed(ThreadStartRequest {
//!         cwd: None,
//!         provider: Some("deepseek-official".into()),
//!         model: None,
//!         reasoning_effort: None,
//!         permission_profile: None,
//!         sandbox_backend: None,
//!     })
//!     .await
//!     .unwrap();
//! let thread_id = thread.thread_id().to_owned();
//! while let Some(event) = thread.recv().await.unwrap() {
//!     println!("{event:?}");
//! }
//! let mut turn = client
//!     .turn_start_streamed(TurnStartRequest {
//!         expected_config_snapshot_id: None,
//!         thread_id,
//!         input: "list the workspace".into(),
//!         provider: None,
//!         model: None,
//!         reasoning_effort: None,
//!         permission_profile: None,
//!         sandbox_backend: None,
//!     })
//!     .await
//!     .unwrap();
//! while let Some(event) = turn.recv().await.unwrap() {
//!     println!("{event:?}");
//! }
//! process.wait().await.unwrap();
//! # })
//! # }
//! ```

#![warn(missing_docs)]

pub mod client;
mod connection;
mod error;
mod initialize;
mod notification;
mod stream;
mod transport;

pub use client::{HarnessClient, HarnessProcess, RequestTimeouts};
pub use error::SdkError;
pub use notification::{HarnessNotification, SequenceStatus};
pub use stream::RequestEvents;
pub use transport::{decode_line, request_line, WireMessage};
