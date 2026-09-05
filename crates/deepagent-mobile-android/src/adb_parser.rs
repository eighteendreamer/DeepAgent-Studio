use serde::{Deserialize, Serialize};

/// Parsed status from `adb devices -l` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdbDeviceStatus {
    Device,
    Unauthorized,
    Offline,
    NoPermissions,
    Bootloader,
    Recovery,
    Sideload,
    Unknown,
}

/// A single entry from `adb devices -l`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdbDeviceEntry {
    pub serial: String,
    pub status: AdbDeviceStatus,
    pub model: Option<String>,
    pub device: Option<String>,
    pub transport_id: Option<String>,
    pub product: Option<String>,
}

/// Parse the output of `adb devices -l`.
///
/// Expected format:
/// ```text
/// List of devices attached
/// ABC123DEF456     device    product:sdk_gphone64_x86_64 model:sdk_gphone64_x86_64 device:emu64xa transport_id:1
/// 192.168.1.100:5555  unauthorized
/// ```
///
/// Lines that don't match the device pattern are silently skipped (header,
/// empty lines, daemon messages).
pub fn parse_adb_devices(output: &str) -> Vec<AdbDeviceEntry> {
    let mut entries = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with("List of devices")
            || line.starts_with("*")
            || line.starts_with("adb:")
        {
            continue;
        }

        if let Some(entry) = parse_device_line(line) {
            entries.push(entry);
        }
    }

    entries
}

fn parse_device_line(line: &str) -> Option<AdbDeviceEntry> {
    let mut parts = line.split_whitespace();
    let serial = parts.next()?;
    let status_str = parts.next()?;

    let status = match status_str {
        "device" => AdbDeviceStatus::Device,
        "unauthorized" => AdbDeviceStatus::Unauthorized,
        "offline" => AdbDeviceStatus::Offline,
        "no" => {
            if parts.next() == Some("permissions") {
                AdbDeviceStatus::NoPermissions
            } else {
                AdbDeviceStatus::Unknown
            }
        }
        "bootloader" => AdbDeviceStatus::Bootloader,
        "recovery" => AdbDeviceStatus::Recovery,
        "sideload" => AdbDeviceStatus::Sideload,
        _ => AdbDeviceStatus::Unknown,
    };

    let mut model = None;
    let mut device = None;
    let mut transport_id = None;
    let mut product = None;

    for kv in parts {
        if let Some((key, value)) = kv.split_once(':') {
            match key {
                "model" => model = Some(value.to_string()),
                "device" => device = Some(value.to_string()),
                "transport_id" => transport_id = Some(value.to_string()),
                "product" => product = Some(value.to_string()),
                _ => {}
            }
        }
    }

    Some(AdbDeviceEntry {
        serial: serial.to_string(),
        status,
        model,
        device,
        transport_id,
        product,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADB_DEVICES_TYPICAL: &str = "\
List of devices attached
ABC123DEF456    device    product:sdk_gphone64_x86_64 model:sdk_gphone64_x86_64 device:emu64xa transport_id:1
192.168.1.100:5555    unauthorized
XYZ789    offline
";

    const ADB_DEVICES_EMPTY: &str = "List of devices attached\n\n";

    const ADB_DEVICES_WITH_DAEMON: &str = "\
* daemon not running; starting now at tcp:5037
* daemon started successfully
List of devices attached
SERIAL001    device    product:walleye model:Pixel_2 device:walleye transport_id:3
";

    const ADB_DEVICES_NO_PERMISSIONS: &str = "\
List of devices attached
AB1234    no permissions (udev rule issue)
";

    #[test]
    fn parse_typical_output() {
        let entries = parse_adb_devices(ADB_DEVICES_TYPICAL);
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].serial, "ABC123DEF456");
        assert_eq!(entries[0].status, AdbDeviceStatus::Device);
        assert_eq!(entries[0].model.as_deref(), Some("sdk_gphone64_x86_64"));
        assert_eq!(entries[0].device.as_deref(), Some("emu64xa"));
        assert_eq!(entries[0].transport_id.as_deref(), Some("1"));

        assert_eq!(entries[1].serial, "192.168.1.100:5555");
        assert_eq!(entries[1].status, AdbDeviceStatus::Unauthorized);

        assert_eq!(entries[2].serial, "XYZ789");
        assert_eq!(entries[2].status, AdbDeviceStatus::Offline);
    }

    #[test]
    fn parse_empty_output() {
        let entries = parse_adb_devices(ADB_DEVICES_EMPTY);
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_with_daemon_messages() {
        let entries = parse_adb_devices(ADB_DEVICES_WITH_DAEMON);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].serial, "SERIAL001");
        assert_eq!(entries[0].status, AdbDeviceStatus::Device);
        assert_eq!(entries[0].product.as_deref(), Some("walleye"));
    }

    #[test]
    fn parse_no_permissions() {
        let entries = parse_adb_devices(ADB_DEVICES_NO_PERMISSIONS);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].serial, "AB1234");
        assert_eq!(entries[0].status, AdbDeviceStatus::NoPermissions);
    }

    #[test]
    fn parse_completely_empty() {
        assert!(parse_adb_devices("").is_empty());
    }

    #[test]
    fn parse_unknown_status() {
        let entries = parse_adb_devices("SERIAL1    weirdstatus\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, AdbDeviceStatus::Unknown);
    }
}
