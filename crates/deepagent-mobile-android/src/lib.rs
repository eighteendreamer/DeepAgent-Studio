//! ADB, Emulator and UI Automator backend for the DeepAgent Mobile subsystem.
//!
//! This crate implements `MobileBackend` from `deepagent-mobile-runtime` using
//! the Android SDK toolchain (`adb`, `emulator`, etc.). All external process
//! calls go through an `AdbCommandRunner` that uses argv arrays (no shell),
//! with timeout and cancellation support.
//!
//! For testing without a real device, a `FakeAndroidBackend` is provided.

mod adb_parser;
mod adb_runner;
mod backend;
mod emulator_parser;
mod emulator_runner;
mod fake;
mod network_capture;
mod tool_resolver;

pub use adb_parser::{parse_adb_devices, AdbDeviceEntry, AdbDeviceStatus};
pub use adb_runner::{AdbCommandOutput, AdbCommandRunner, FakeAdbRunner, SystemAdbRunner};
pub use backend::AdbBackend;
pub use emulator_parser::{extract_emulator_port, parse_avdmanager_list, parse_list_avds};
pub use emulator_runner::{EmulatorCommandRunner, FakeEmulatorRunner, SystemEmulatorRunner};
pub use fake::FakeAndroidBackend;
pub use tool_resolver::ToolResolver;
