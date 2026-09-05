//! simctl, devicectl and XCTest backend for the DeepAgent Mobile subsystem.
//!
//! This crate provides iOS/macOS device management using Apple's toolchain
//! (`xcrun simctl`, `xcrun devicectl`). On non-macOS platforms, the tool
//! resolver reports unavailability and directs users to the Remote Mac runtime.
//!
//! For testing without a real Mac, a `FakeIosBackend` is provided.

mod fake;
mod simctl_parser;
mod tool_resolver;

pub use fake::FakeIosBackend;
pub use simctl_parser::{parse_simctl_devices, parse_simctl_list, parse_simctl_runtimes};
pub use tool_resolver::IosToolResolver;
