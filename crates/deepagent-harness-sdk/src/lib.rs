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
mod error;
mod stream;
mod transport;

pub use client::{HarnessClient, HarnessProcess, RequestTimeouts};
pub use error::SdkError;
pub use stream::RequestEvents;
pub use transport::{decode_line, request_line, WireMessage};
