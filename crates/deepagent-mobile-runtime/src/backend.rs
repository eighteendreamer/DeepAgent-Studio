use async_trait::async_trait;
use deepagent_mobile_core::{ArtifactRef, BackendStatus, MobileDevice, MobileResult};
use deepagent_mobile_protocol::{
    AppTarget, AvdInfo, InputRequest, InputResult, InstallRequest, LaunchRequest, LogPage,
    LogRequest, StartEmulatorRequest, StopEmulatorRequest, UiSnapshot,
};

use crate::OperationContext;

/// Platform backend trait.
///
/// Implementations wrap external tools (ADB, simctl, devicectl, XCTest, etc.)
/// and translate their output into the unified mobile type system. Backends
/// must **not** perform permission checks, event persistence, or agent tool
/// registration — those belong to higher layers.
///
/// All methods accept `&OperationContext` for cancellation and deadline
/// enforcement. Implementations must check `ctx.is_cancelled()` before each
/// external call and return `MobileError::Cancelled` promptly.
#[async_trait]
pub trait MobileBackend: Send + Sync {
    /// Probe the toolchain and report availability.
    async fn probe(&self) -> MobileResult<BackendStatus>;

    /// List currently visible devices.
    async fn list_devices(&self, ctx: &OperationContext) -> MobileResult<Vec<MobileDevice>>;

    /// Return detailed information for a single device.
    async fn device_info(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<MobileDevice>;

    /// Capture a screenshot and return an artifact reference.
    async fn screenshot(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<ArtifactRef>;

    /// Capture the full UI hierarchy snapshot.
    async fn ui_snapshot(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<UiSnapshot>;

    /// Install an application.
    async fn install(&self, request: &InstallRequest, ctx: &OperationContext) -> MobileResult<()>;

    /// Launch an application.
    async fn launch(&self, request: &LaunchRequest, ctx: &OperationContext) -> MobileResult<()>;

    /// Terminate a running application.
    async fn terminate(&self, target: &AppTarget, ctx: &OperationContext) -> MobileResult<()>;

    /// Perform a structured input action.
    async fn input(
        &self,
        request: &InputRequest,
        ctx: &OperationContext,
    ) -> MobileResult<InputResult>;

    /// Read device logs.
    async fn read_logs(
        &self,
        request: &LogRequest,
        ctx: &OperationContext,
    ) -> MobileResult<LogPage>;

    /// List available Android Virtual Devices (AVDs).
    ///
    /// Returns an empty list for non-Android backends.
    async fn list_avds(&self, ctx: &OperationContext) -> MobileResult<Vec<AvdInfo>> {
        let _ = ctx;
        Ok(vec![])
    }

    /// Start an Android Emulator.
    ///
    /// Returns the ADB serial of the started emulator (e.g., "emulator-5554").
    /// Default implementation returns an error for backends that don't support
    /// emulators.
    async fn start_emulator(
        &self,
        request: &StartEmulatorRequest,
        ctx: &OperationContext,
    ) -> MobileResult<String> {
        let _ = (request, ctx);
        Err(deepagent_mobile_core::MobileError::NotSupported {
            operation: "start_emulator".into(),
        })
    }

    /// Stop a running Android Emulator.
    async fn stop_emulator(
        &self,
        request: &StopEmulatorRequest,
        ctx: &OperationContext,
    ) -> MobileResult<()> {
        let _ = (request, ctx);
        Err(deepagent_mobile_core::MobileError::NotSupported {
            operation: "stop_emulator".into(),
        })
    }

    /// Start capturing network traffic for a device.
    async fn start_network_capture(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<()> {
        let _ = (device_id, ctx);
        Err(deepagent_mobile_core::MobileError::NotSupported {
            operation: "start_network_capture".into(),
        })
    }

    /// Stop capturing network traffic for a device.
    async fn stop_network_capture(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<()> {
        let _ = (device_id, ctx);
        Err(deepagent_mobile_core::MobileError::NotSupported {
            operation: "stop_network_capture".into(),
        })
    }

    /// Get captured network records for a device.
    async fn get_network_records(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<Vec<deepagent_mobile_protocol::NetworkRecord>> {
        let _ = (device_id, ctx);
        Err(deepagent_mobile_core::MobileError::NotSupported {
            operation: "get_network_records".into(),
        })
    }
}
