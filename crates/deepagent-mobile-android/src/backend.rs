use async_trait::async_trait;
use deepagent_mobile_core::*;
use deepagent_mobile_protocol::*;
use deepagent_mobile_runtime::{MobileBackend, OperationContext};
use std::sync::Arc;

use crate::adb_parser::{parse_adb_devices, AdbDeviceStatus};
use crate::adb_runner::AdbCommandRunner;

/// Real ADB-based Android backend.
///
/// All process calls go through the injected `AdbCommandRunner`, which uses
/// argv arrays (no shell), supports timeout and cancellation.
pub struct AdbBackend {
    resolver: super::ToolResolver,
    runner: Arc<dyn AdbCommandRunner>,
}

impl AdbBackend {
    pub fn new(resolver: super::ToolResolver, runner: Arc<dyn AdbCommandRunner>) -> Self {
        Self { resolver, runner }
    }

    fn adb_path(&self) -> MobileResult<String> {
        self.resolver
            .resolve("adb")
            .map(|t| t.path.display().to_string())
            .ok_or_else(|| MobileError::ToolNotFound {
                tool_name: "adb".into(),
            })
    }

    fn map_device_state(status: AdbDeviceStatus) -> DeviceState {
        match status {
            AdbDeviceStatus::Device => DeviceState::Ready,
            AdbDeviceStatus::Unauthorized => DeviceState::Unauthorized,
            AdbDeviceStatus::Offline => DeviceState::Offline,
            AdbDeviceStatus::NoPermissions => DeviceState::Error,
            AdbDeviceStatus::Bootloader | AdbDeviceStatus::Recovery | AdbDeviceStatus::Sideload => {
                DeviceState::Booting
            }
            AdbDeviceStatus::Unknown => DeviceState::Error,
        }
    }

    fn device_id_from_serial(serial: &str) -> String {
        format!("android-{serial}")
    }

    fn entry_to_device(entry: &crate::AdbDeviceEntry) -> MobileDevice {
        let is_emulator = entry.serial.contains("emulator") || entry.serial.contains(':');
        MobileDevice {
            id: Self::device_id_from_serial(&entry.serial),
            name: entry.model.clone().unwrap_or_else(|| entry.serial.clone()),
            platform: MobilePlatform::Android,
            kind: if is_emulator {
                DeviceKind::Emulator
            } else {
                DeviceKind::Physical
            },
            connection: if entry.serial.contains(':') && !entry.serial.contains("emulator") {
                DeviceConnection::Remote {
                    host_id: entry.serial.clone(),
                }
            } else {
                DeviceConnection::Usb
            },
            state: Self::map_device_state(entry.status),
            os_version: None,
            capabilities: DeviceCapabilities {
                screenshot: entry.status == AdbDeviceStatus::Device,
                ui_tree: entry.status == AdbDeviceStatus::Device,
                input: entry.status == AdbDeviceStatus::Device,
                logs: entry.status == AdbDeviceStatus::Device,
                install: entry.status == AdbDeviceStatus::Device,
                network_inspection: false,
            },
        }
    }

    async fn adb_shell(
        &self,
        serial: &str,
        shell_args: &[&str],
        ctx: &OperationContext,
    ) -> MobileResult<crate::adb_runner::AdbCommandOutput> {
        let adb = self.adb_path()?;
        let mut args: Vec<&str> = vec!["-s", serial, "shell"];
        args.extend_from_slice(shell_args);
        self.runner
            .run(&adb, &args, ctx.deadline, &ctx.cancellation_token())
            .await
    }

    fn check_exit(output: &crate::adb_runner::AdbCommandOutput, tool: &str) -> MobileResult<()> {
        match output.exit_code {
            Some(0) => Ok(()),
            Some(code) => Err(MobileError::ToolExecutionFailed {
                tool_name: tool.into(),
                exit_code: code,
                stderr: output.stderr.clone(),
            }),
            None => Err(MobileError::ToolExecutionFailed {
                tool_name: tool.into(),
                exit_code: -1,
                stderr: "process killed by signal".into(),
            }),
        }
    }
}

#[async_trait]
impl MobileBackend for AdbBackend {
    async fn probe(&self) -> MobileResult<BackendStatus> {
        let adb = self.resolver.resolve("adb");
        let emulator = self.resolver.resolve("emulator");

        let mut tool_paths = Vec::new();
        let mut diagnostics = Vec::new();

        if let Some(ref t) = adb {
            tool_paths.push(ToolPath {
                name: "adb".into(),
                path: t.path.display().to_string(),
                version: t.version.clone(),
            });
        } else {
            diagnostics.push("adb not found in PATH".into());
        }

        if let Some(ref t) = emulator {
            tool_paths.push(ToolPath {
                name: "emulator".into(),
                path: t.path.display().to_string(),
                version: t.version.clone(),
            });
        } else {
            diagnostics.push("emulator not found".into());
        }

        Ok(BackendStatus {
            platform: MobilePlatform::Android,
            available: adb.is_some(),
            toolchain_version: adb.and_then(|t| t.version),
            tool_paths,
            diagnostics,
        })
    }

    async fn list_devices(&self, ctx: &OperationContext) -> MobileResult<Vec<MobileDevice>> {
        let adb = self.adb_path()?;
        let output = self
            .runner
            .run(
                &adb,
                &["devices", "-l"],
                ctx.deadline,
                &ctx.cancellation_token(),
            )
            .await?;
        Self::check_exit(&output, "adb devices")?;

        let entries = parse_adb_devices(&output.stdout);
        Ok(entries.iter().map(Self::entry_to_device).collect())
    }

    async fn device_info(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<MobileDevice> {
        let serial =
            device_id
                .strip_prefix("android-")
                .ok_or_else(|| MobileError::DeviceNotFound {
                    device_id: device_id.into(),
                })?;

        let output = self
            .adb_shell(serial, &["getprop", "ro.product.model"], ctx)
            .await?;
        Self::check_exit(&output, "adb shell getprop")?;

        let model = output.stdout.trim().to_string();

        let version_output = self
            .adb_shell(serial, &["getprop", "ro.build.version.release"], ctx)
            .await?;
        let os_version = Some(version_output.stdout.trim().to_string());

        let mut device = MobileDevice {
            id: device_id.into(),
            name: if model.is_empty() {
                serial.into()
            } else {
                model
            },
            platform: MobilePlatform::Android,
            kind: if serial.contains("emulator") || serial.contains(':') {
                DeviceKind::Emulator
            } else {
                DeviceKind::Physical
            },
            connection: DeviceConnection::Usb,
            state: DeviceState::Ready,
            os_version,
            capabilities: DeviceCapabilities {
                screenshot: true,
                ui_tree: true,
                input: true,
                logs: true,
                install: true,
                network_inspection: false,
            },
        };

        if serial.contains(':') && !serial.contains("emulator") {
            device.connection = DeviceConnection::Remote {
                host_id: serial.into(),
            };
        }

        Ok(device)
    }

    async fn screenshot(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<ArtifactRef> {
        let serial =
            device_id
                .strip_prefix("android-")
                .ok_or_else(|| MobileError::DeviceNotFound {
                    device_id: device_id.into(),
                })?;

        let output = self.adb_shell(serial, &["screencap", "-p"], ctx).await?;
        Self::check_exit(&output, "adb screencap")?;

        let size = output.stdout.len() as u64;
        Ok(ArtifactRef {
            artifact_id: format!("screenshot-{device_id}-{}", uuid::Uuid::new_v4()),
            mime: "image/png".into(),
            size_bytes: size,
            sha256: None,
            storage_path: format!("memory://{device_id}/screenshot.png"),
        })
    }

    async fn ui_snapshot(
        &self,
        device_id: &str,
        ctx: &OperationContext,
    ) -> MobileResult<UiSnapshot> {
        let serial =
            device_id
                .strip_prefix("android-")
                .ok_or_else(|| MobileError::DeviceNotFound {
                    device_id: device_id.into(),
                })?;

        let output = self
            .adb_shell(serial, &["uiautomator", "dump", "/dev/tty"], ctx)
            .await?;
        Self::check_exit(&output, "adb uiautomator dump")?;

        let snapshot_id = format!("snap-{device_id}-{}", uuid::Uuid::new_v4());
        let nodes = parse_uiautomator_output(&output.stdout);

        Ok(UiSnapshot {
            snapshot_id,
            device_id: device_id.into(),
            root_node_id: nodes
                .first()
                .map(|n| n.node_id.clone())
                .unwrap_or_else(|| "root".into()),
            captured_at_ms: 0,
            nodes,
        })
    }

    async fn install(&self, request: &InstallRequest, ctx: &OperationContext) -> MobileResult<()> {
        let serial = request.device_id.strip_prefix("android-").ok_or_else(|| {
            MobileError::DeviceNotFound {
                device_id: request.device_id.clone(),
            }
        })?;

        let adb = self.adb_path()?;
        let output = self
            .runner
            .run(
                &adb,
                &["-s", serial, "install", &request.artifact_path],
                ctx.deadline,
                &ctx.cancellation_token(),
            )
            .await?;
        Self::check_exit(&output, "adb install")
    }

    async fn launch(&self, request: &LaunchRequest, ctx: &OperationContext) -> MobileResult<()> {
        let serial = request.device_id.strip_prefix("android-").ok_or_else(|| {
            MobileError::DeviceNotFound {
                device_id: request.device_id.clone(),
            }
        })?;

        let component = match &request.activity {
            Some(activity) => format!("{}/{}", request.package, activity),
            None => format!("{}/{}.MainActivity", request.package, request.package),
        };

        let output = self
            .adb_shell(serial, &["am", "start", "-n", &component], ctx)
            .await?;
        Self::check_exit(&output, "adb am start")
    }

    async fn terminate(&self, target: &AppTarget, ctx: &OperationContext) -> MobileResult<()> {
        let serial = target.device_id.strip_prefix("android-").ok_or_else(|| {
            MobileError::DeviceNotFound {
                device_id: target.device_id.clone(),
            }
        })?;

        let output = self
            .adb_shell(serial, &["am", "force-stop", &target.package], ctx)
            .await?;
        Self::check_exit(&output, "adb am force-stop")
    }

    async fn input(
        &self,
        request: &InputRequest,
        ctx: &OperationContext,
    ) -> MobileResult<InputResult> {
        let serial = request.device_id.strip_prefix("android-").ok_or_else(|| {
            MobileError::DeviceNotFound {
                device_id: request.device_id.clone(),
            }
        })?;

        let args: Vec<String> = match &request.action {
            InputAction::Tap { x, y } => {
                vec!["input".into(), "tap".into(), x.to_string(), y.to_string()]
            }
            InputAction::LongPress { x, y, duration_ms } => vec![
                "input".into(),
                "swipe".into(),
                x.to_string(),
                y.to_string(),
                x.to_string(),
                y.to_string(),
                duration_ms.to_string(),
            ],
            InputAction::Swipe {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            } => vec![
                "input".into(),
                "swipe".into(),
                x1.to_string(),
                y1.to_string(),
                x2.to_string(),
                y2.to_string(),
                duration_ms.to_string(),
            ],
            InputAction::InputText { text } => {
                let escaped = text.replace(' ', "%s");
                vec!["input".into(), "text".into(), escaped]
            }
            InputAction::PressBack => {
                vec!["input".into(), "keyevent".into(), "KEYCODE_BACK".into()]
            }
        };

        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let output = self.adb_shell(serial, &arg_refs, ctx).await?;
        Self::check_exit(&output, "adb input")?;
        Ok(InputResult { accepted: true })
    }

    async fn read_logs(
        &self,
        request: &LogRequest,
        ctx: &OperationContext,
    ) -> MobileResult<LogPage> {
        let serial = request.device_id.strip_prefix("android-").ok_or_else(|| {
            MobileError::DeviceNotFound {
                device_id: request.device_id.clone(),
            }
        })?;

        let max = request.max_lines.min(1000);
        let output = self
            .adb_shell(serial, &["logcat", "-d", "-t", &max.to_string()], ctx)
            .await?;
        Self::check_exit(&output, "adb logcat")?;

        let records = output
            .stdout
            .lines()
            .filter(|l| !l.is_empty())
            .map(|line| {
                let (level, tag, message) = parse_logcat_line(line);
                LogRecord {
                    timestamp_ms: 0,
                    level,
                    tag,
                    message,
                }
            })
            .collect::<Vec<_>>();

        Ok(LogPage {
            device_id: request.device_id.clone(),
            records,
            truncated: output.stdout.lines().count() > max as usize,
        })
    }

    async fn list_avds(&self, ctx: &OperationContext) -> MobileResult<Vec<AvdInfo>> {
        if ctx.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: ctx.operation_id.clone(),
            });
        }

        let emulator_path = self
            .resolver
            .resolve("emulator")
            .map(|t| t.path.display().to_string())
            .ok_or_else(|| MobileError::ToolNotFound {
                tool_name: "emulator".into(),
            })?;

        let output = self
            .runner
            .run(
                &emulator_path,
                &["-list-avds"],
                ctx.deadline,
                &ctx.cancellation_token(),
            )
            .await?;

        if output.exit_code != Some(0) {
            return Err(MobileError::ToolExecutionFailed {
                tool_name: "emulator".into(),
                exit_code: output.exit_code.unwrap_or(-1),
                stderr: output.stderr,
            });
        }

        let mut avds = crate::emulator_parser::parse_list_avds(&output.stdout);

        // Mark running AVDs by checking current devices
        let devices = self.list_devices(ctx).await.unwrap_or_default();
        for avd in &mut avds {
            for device in &devices {
                if device.kind == DeviceKind::Emulator {
                    // Check if this emulator matches the AVD name
                    // Emulator serials are like "emulator-5554"
                    if let Some(serial) = device.id.strip_prefix("android-emulator-") {
                        avd.running = true;
                        avd.serial = Some(serial.to_string());
                        break;
                    }
                }
            }
        }

        Ok(avds)
    }

    async fn start_emulator(
        &self,
        request: &StartEmulatorRequest,
        ctx: &OperationContext,
    ) -> MobileResult<String> {
        if ctx.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: ctx.operation_id.clone(),
            });
        }

        let emulator_path = self
            .resolver
            .resolve("emulator")
            .map(|t| t.path.display().to_string())
            .ok_or_else(|| MobileError::ToolNotFound {
                tool_name: "emulator".into(),
            })?;

        // Build emulator command: emulator -avd <name> [args...]
        let mut args: Vec<&str> = vec!["-avd", &request.avd_name];
        let extra_args: Vec<&str> = request.args.iter().map(|s| s.as_str()).collect();
        args.extend_from_slice(&extra_args);

        // Start emulator in background (don't wait for it to exit)
        // The emulator process will keep running after this command returns
        let _output = self
            .runner
            .run(
                &emulator_path,
                &args,
                std::time::Duration::from_millis(request.boot_timeout_ms),
                &ctx.cancellation_token(),
            )
            .await?;

        // Wait for the emulator to appear in adb devices
        let boot_deadline = std::time::Duration::from_millis(request.boot_timeout_ms);
        let poll_interval = std::time::Duration::from_secs(2);
        let start = std::time::Instant::now();

        while start.elapsed() < boot_deadline {
            if ctx.is_cancelled() {
                return Err(MobileError::Cancelled {
                    operation_id: ctx.operation_id.clone(),
                });
            }

            tokio::time::sleep(poll_interval).await;

            // Check if emulator is online
            let devices = self.list_devices(ctx).await.unwrap_or_default();
            for device in devices {
                if device.kind == DeviceKind::Emulator && device.state == DeviceState::Ready {
                    // Extract serial from device id (android-emulator-5554 -> emulator-5554)
                    if let Some(serial) = device.id.strip_prefix("android-") {
                        tracing::info!(
                            avd_name = %request.avd_name,
                            serial = %serial,
                            "emulator started"
                        );
                        return Ok(serial.to_string());
                    }
                }
            }
        }

        Err(MobileError::Timeout {
            operation_id: ctx.operation_id.clone(),
            elapsed_ms: request.boot_timeout_ms,
        })
    }

    async fn stop_emulator(
        &self,
        request: &StopEmulatorRequest,
        ctx: &OperationContext,
    ) -> MobileResult<()> {
        if ctx.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: ctx.operation_id.clone(),
            });
        }

        // Use adb emu kill to stop the emulator
        let output = self
            .adb_shell(&request.serial, &["emu", "kill"], ctx)
            .await?;

        // emu kill may not return a clean exit code, but the emulator will stop
        tracing::info!(serial = %request.serial, exit_code = ?output.exit_code, "emulator stop requested");
        Ok(())
    }
}

fn parse_logcat_line(line: &str) -> (String, Option<String>, String) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 6 {
        let level = parts[4].to_string();
        let tag = Some(parts[5].trim_end_matches(':').to_string());
        let message = parts[6..].join(" ");
        (level, tag, message)
    } else {
        ("I".into(), None, line.to_string())
    }
}

fn parse_uiautomator_output(output: &str) -> Vec<UiNode> {
    let xml_start = output.find("<?xml");
    let xml = match xml_start {
        Some(start) => &output[start..],
        None => {
            let node = UiNode {
                node_id: "root".into(),
                parent_id: None,
                role: UiRole::Page,
                text: None,
                label: None,
                accessibility_id: None,
                bounds: Bounds {
                    x: 0,
                    y: 0,
                    width: 1080,
                    height: 1920,
                },
                visible: true,
                enabled: true,
                clickable: false,
                editable: false,
                children: vec![],
                source: UiNodeSource::AndroidUiAutomator,
            };
            return vec![node];
        }
    };

    let mut nodes = Vec::new();
    let mut node_counter = 0u32;

    for segment in xml.split("<node") {
        if !segment.contains("bounds=") {
            continue;
        }

        let node_id = format!("ui-{node_counter}");
        node_counter += 1;

        let text = extract_attr(segment, "text");
        let resource_id = extract_attr(segment, "resource-id");
        let class = extract_attr(segment, "class");
        let content_desc = extract_attr(segment, "content-desc");
        let visible = extract_attr(segment, "visible-to-user")
            .map(|v| v != "false")
            .unwrap_or(true);
        let enabled = extract_attr(segment, "enabled")
            .map(|v| v != "false")
            .unwrap_or(true);
        let clickable = extract_attr(segment, "clickable")
            .map(|v| v == "true")
            .unwrap_or(false);
        let editable = extract_attr(segment, "editable")
            .map(|v| v == "true")
            .unwrap_or(false);

        let bounds = parse_bounds_attr(segment);
        let role = infer_role(&class, &resource_id, &text);

        nodes.push(UiNode {
            node_id,
            parent_id: None,
            role,
            text,
            label: content_desc,
            accessibility_id: None,
            bounds,
            visible,
            enabled,
            clickable,
            editable,
            children: vec![],
            source: UiNodeSource::AndroidUiAutomator,
        });
    }

    if nodes.is_empty() {
        nodes.push(UiNode {
            node_id: "root".into(),
            parent_id: None,
            role: UiRole::Page,
            text: None,
            label: None,
            accessibility_id: None,
            bounds: Bounds {
                x: 0,
                y: 0,
                width: 1080,
                height: 1920,
            },
            visible: true,
            enabled: true,
            clickable: false,
            editable: false,
            children: vec![],
            source: UiNodeSource::AndroidUiAutomator,
        });
    }

    nodes
}

fn extract_attr(segment: &str, attr: &str) -> Option<String> {
    let pattern = format!("{attr}=\"");
    let start = segment.find(&pattern)?;
    let value_start = start + pattern.len();
    let end = segment[value_start..].find('"')?;
    let value = &segment[value_start..value_start + end];
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_bounds_attr(segment: &str) -> Bounds {
    let bounds_str = extract_attr(segment, "bounds").unwrap_or_default();
    let parts: Vec<&str> = bounds_str.split(|c: char| !c.is_ascii_digit()).collect();
    let nums: Vec<u32> = parts
        .iter()
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect();

    if nums.len() >= 4 {
        Bounds {
            x: nums[0],
            y: nums[1],
            width: nums[2].saturating_sub(nums[0]),
            height: nums[3].saturating_sub(nums[1]),
        }
    } else {
        Bounds {
            x: 0,
            y: 0,
            width: 1080,
            height: 1920,
        }
    }
}

fn infer_role(
    class: &Option<String>,
    _resource_id: &Option<String>,
    text: &Option<String>,
) -> UiRole {
    let cls = class.as_deref().unwrap_or("");
    if cls.contains("Button") || cls.contains("ImageButton") {
        UiRole::Button
    } else if cls.contains("EditText") {
        UiRole::TextBox
    } else if cls.contains("CheckBox") {
        UiRole::Checkbox
    } else if cls.contains("Switch") {
        UiRole::Switch
    } else if cls.contains("ImageView") {
        UiRole::Image
    } else if cls.contains("ListView") || cls.contains("RecyclerView") {
        UiRole::List
    } else if cls.contains("WebView") {
        UiRole::WebView
    } else if cls.contains("Dialog") || cls.contains("PopupWindow") {
        UiRole::Dialog
    } else if text.is_some() {
        UiRole::Text
    } else {
        UiRole::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adb_runner::{AdbCommandOutput, FakeAdbRunner};
    use crate::tool_resolver::ResolvedTool;
    use std::time::Duration;

    fn test_backend() -> (AdbBackend, Arc<FakeAdbRunner>) {
        let runner = Arc::new(FakeAdbRunner::new());
        let resolver = super::super::ToolResolver::new();
        resolver.insert(ResolvedTool {
            name: "adb".into(),
            path: std::path::PathBuf::from("/fake/adb"),
            version: Some("1.0.41".into()),
        });
        let backend = AdbBackend::new(resolver, runner.clone());
        (backend, runner)
    }

    fn ctx() -> OperationContext {
        OperationContext::new(
            "op-test".into(),
            "android-ABC123".into(),
            Duration::from_secs(30),
        )
    }

    #[tokio::test]
    async fn list_devices_parses_adb_output() {
        let (backend, runner) = test_backend();
        runner
            .set_output(
                "devices",
                AdbCommandOutput {
                    exit_code: Some(0),
                    stdout: "List of devices attached\nABC123\tdevice\tproduct:walleye model:Pixel_2 device:walleye transport_id:1\n".into(),
                    stderr: String::new(),
                },
            )
            .await;

        let devices = backend.list_devices(&ctx()).await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, "android-ABC123");
        assert_eq!(devices[0].state, DeviceState::Ready);
        assert_eq!(devices[0].kind, DeviceKind::Physical);
    }

    #[tokio::test]
    async fn list_devices_handles_unauthorized() {
        let (backend, runner) = test_backend();
        runner
            .set_output(
                "devices",
                AdbCommandOutput {
                    exit_code: Some(0),
                    stdout: "List of devices attached\nXYZ789\tunauthorized\n".into(),
                    stderr: String::new(),
                },
            )
            .await;

        let devices = backend.list_devices(&ctx()).await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].state, DeviceState::Unauthorized);
    }

    #[tokio::test]
    async fn list_devices_handles_offline() {
        let (backend, runner) = test_backend();
        runner
            .set_output(
                "devices",
                AdbCommandOutput {
                    exit_code: Some(0),
                    stdout: "List of devices attached\nOFF001\toffline\n".into(),
                    stderr: String::new(),
                },
            )
            .await;

        let devices = backend.list_devices(&ctx()).await.unwrap();
        assert_eq!(devices[0].state, DeviceState::Offline);
    }

    #[tokio::test]
    async fn list_devices_emulator_detected() {
        let (backend, runner) = test_backend();
        runner
            .set_output(
                "devices",
                AdbCommandOutput {
                    exit_code: Some(0),
                    stdout: "List of devices attached\nemulator-5554\tdevice\tmodel:sdk_gphone64\n"
                        .to_string(),
                    stderr: String::new(),
                },
            )
            .await;

        let devices = backend.list_devices(&ctx()).await.unwrap();
        assert_eq!(devices[0].kind, DeviceKind::Emulator);
    }

    #[tokio::test]
    async fn launch_builds_correct_argv() {
        let (backend, runner) = test_backend();
        runner
            .set_output(
                "shell",
                AdbCommandOutput {
                    exit_code: Some(0),
                    stdout: "Starting: Intent { ... }\n".into(),
                    stderr: String::new(),
                },
            )
            .await;

        let result = backend
            .launch(
                &LaunchRequest {
                    device_id: "android-ABC123".into(),
                    package: "com.example.app".into(),
                    activity: Some(".MainActivity".into()),
                },
                &ctx(),
            )
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn input_tap_accepted() {
        let (backend, runner) = test_backend();
        runner
            .set_output(
                "shell",
                AdbCommandOutput {
                    exit_code: Some(0),
                    stdout: String::new(),
                    stderr: String::new(),
                },
            )
            .await;

        let result = backend
            .input(
                &InputRequest {
                    device_id: "android-ABC123".into(),
                    snapshot_id: None,
                    action: InputAction::Tap { x: 100, y: 200 },
                },
                &ctx(),
            )
            .await
            .unwrap();
        assert!(result.accepted);
    }

    #[tokio::test]
    async fn adb_tool_not_found() {
        let resolver = super::super::ToolResolver::new();
        let runner = Arc::new(FakeAdbRunner::new());
        let backend = AdbBackend::new(resolver, runner);
        let err = backend.list_devices(&ctx()).await.unwrap_err();
        assert!(matches!(err, MobileError::ToolNotFound { .. }));
    }

    #[test]
    fn parse_logcat_line_full() {
        let (level, tag, msg) =
            parse_logcat_line("09-05 12:00:00.000  1234  5678 I SystemServer: Start");
        assert_eq!(level, "I");
        assert_eq!(tag.as_deref(), Some("SystemServer"));
        assert_eq!(msg, "Start");
    }

    #[test]
    fn parse_uiautomator_extracts_nodes() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<hierarchy rotation="0">
  <node bounds="[0,0][1080,1920]" class="android.widget.FrameLayout" enabled="true" clickable="false">
    <node bounds="[100,200][300,260]" class="android.widget.Button" enabled="true" clickable="true" text="OK" />
  </node>
</hierarchy>"#;
        let nodes = parse_uiautomator_output(xml);
        assert!(nodes.len() >= 2);
        let btn = nodes.iter().find(|n| n.role == UiRole::Button).unwrap();
        assert_eq!(btn.text.as_deref(), Some("OK"));
        assert!(btn.clickable);
    }

    #[test]
    fn infer_role_button() {
        assert_eq!(
            infer_role(&Some("android.widget.Button".into()), &None, &None),
            UiRole::Button
        );
    }

    #[test]
    fn infer_role_text() {
        assert_eq!(
            infer_role(
                &Some("android.widget.TextView".into()),
                &None,
                &Some("Hello".into())
            ),
            UiRole::Text
        );
    }
}
