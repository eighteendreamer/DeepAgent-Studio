use async_trait::async_trait;
use deepagent_mobile_core::{ArtifactRef, BackendStatus, MobileDevice, MobileResult};
use deepagent_mobile_protocol::{
    AppTarget, InputRequest, InputResult, InstallRequest, LaunchRequest, LogPage, LogRequest,
    UiSnapshot,
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
}
