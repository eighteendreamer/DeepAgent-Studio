//! # deepagent-app-core
//!
//! The application-service façade (开发计划.md Phase 8 backing layer).
//!
//! This crate is the stable boundary between the kernel and any UI. It exposes:
//! - [`dto`] — plain serializable shapes the frontend consumes,
//! - [`service::AppService`] — DTO-returning operations (list sessions, open a
//!   session with its timeline + stats).
//!
//! Tauri commands and a potential web backend are thin wrappers over
//! [`service::AppService`]; the UI never touches kernel internals, so the wire
//! contract stays stable as the kernel evolves.

pub mod dto;
pub mod service;

pub use dto::{SessionDetailDto, SessionStatsDto, SessionSummaryDto, TimelineEntryDto};
pub use service::AppService;
