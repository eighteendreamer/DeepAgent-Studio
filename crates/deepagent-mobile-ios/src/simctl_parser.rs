use deepagent_mobile_protocol::{SimDevice, SimDeviceState, SimRuntime, SimctlListOutput};
use serde::Deserialize;
use std::collections::HashMap;

/// Raw JSON structure from `xcrun simctl list devices --json`.
#[derive(Debug, Deserialize)]
struct RawSimctlDevices {
    #[serde(default)]
    devices: HashMap<String, Vec<RawSimDevice>>,
}

#[derive(Debug, Deserialize)]
struct RawSimDevice {
    udid: String,
    name: String,
    #[serde(default)]
    state: Option<String>,
    #[serde(default, rename = "isAvailable")]
    is_available: Option<bool>,
    #[serde(default, rename = "isUsable")]
    is_usable: Option<bool>,
    #[serde(default, rename = "deviceTypeIdentifier")]
    device_type_id: Option<String>,
}

/// Raw JSON structure from `xcrun simctl list runtimes --json`.
#[derive(Debug, Deserialize)]
struct RawSimctlRuntimes {
    #[serde(default)]
    runtimes: Vec<RawSimRuntime>,
}

#[derive(Debug, Deserialize)]
struct RawSimRuntime {
    identifier: String,
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default, rename = "isAvailable")]
    is_available: Option<bool>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default, rename = "platformPath")]
    _platform_path: Option<String>,
}

/// Parse `xcrun simctl list devices --json` output into `SimDevice` list.
pub fn parse_simctl_devices(json: &str) -> Result<Vec<SimDevice>, String> {
    let raw: RawSimctlDevices =
        serde_json::from_str(json).map_err(|e| format!("failed to parse simctl devices: {e}"))?;

    let mut devices = Vec::new();
    for (runtime_id, raw_devices) in &raw.devices {
        for raw in raw_devices {
            let state = parse_device_state(raw.state.as_deref());
            let is_available = raw.is_available.unwrap_or(true) && raw.is_usable.unwrap_or(true);

            devices.push(SimDevice {
                udid: raw.udid.clone(),
                name: raw.name.clone(),
                state,
                is_available,
                runtime_id: Some(runtime_id.clone()),
                device_type_id: raw.device_type_id.clone(),
            });
        }
    }

    Ok(devices)
}

/// Parse `xcrun simctl list runtimes --json` output into `SimRuntime` list.
pub fn parse_simctl_runtimes(json: &str) -> Result<Vec<SimRuntime>, String> {
    let raw: RawSimctlRuntimes =
        serde_json::from_str(json).map_err(|e| format!("failed to parse simctl runtimes: {e}"))?;

    Ok(raw
        .runtimes
        .into_iter()
        .map(|r| SimRuntime {
            identifier: r.identifier,
            name: r.name,
            version: r.version.unwrap_or_default(),
            is_available: r.is_available.unwrap_or(false),
            platform: r.platform,
        })
        .collect())
}

/// Parse combined `xcrun simctl list --json` output (devices + runtimes).
pub fn parse_simctl_list(json: &str) -> Result<SimctlListOutput, String> {
    let devices = parse_simctl_devices(json)?;
    let runtimes = parse_simctl_runtimes(json)?;
    Ok(SimctlListOutput { devices, runtimes })
}

fn parse_device_state(raw: Option<&str>) -> SimDeviceState {
    match raw {
        Some("Booted") => SimDeviceState::Booted,
        Some("Shutdown") => SimDeviceState::Shutdown,
        Some("Creating") | Some("Booting") => SimDeviceState::Creating,
        _ => SimDeviceState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMCTL_DEVICES_JSON: &str = r#"{
        "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-17-0": [
                {
                    "udid": "ABCD-1234-EFGH",
                    "name": "iPhone 15",
                    "state": "Shutdown",
                    "isAvailable": true,
                    "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-15"
                },
                {
                    "udid": "IJKL-5678-MNOP",
                    "name": "iPhone 15 Pro",
                    "state": "Booted",
                    "isAvailable": true,
                    "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-15-Pro"
                }
            ],
            "com.apple.CoreSimulator.SimRuntime.iOS-16-4": [
                {
                    "udid": "QRST-9012-UVWX",
                    "name": "iPhone 14",
                    "state": "Shutdown",
                    "isAvailable": false,
                    "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-14"
                }
            ]
        }
    }"#;

    const SIMCTL_RUNTIMES_JSON: &str = r#"{
        "runtimes": [
            {
                "identifier": "com.apple.CoreSimulator.SimRuntime.iOS-17-0",
                "name": "iOS 17.0",
                "version": "17.0",
                "isAvailable": true,
                "platform": "iOS"
            },
            {
                "identifier": "com.apple.CoreSimulator.SimRuntime.iOS-16-4",
                "name": "iOS 16.4",
                "version": "16.4",
                "isAvailable": true,
                "platform": "iOS"
            }
        ]
    }"#;

    #[test]
    fn parse_devices_returns_all() {
        let devices = parse_simctl_devices(SIMCTL_DEVICES_JSON).unwrap();
        assert_eq!(devices.len(), 3);
    }

    #[test]
    fn parse_device_fields() {
        let devices = parse_simctl_devices(SIMCTL_DEVICES_JSON).unwrap();
        let iphone15 = devices.iter().find(|d| d.name == "iPhone 15").unwrap();
        assert_eq!(iphone15.udid, "ABCD-1234-EFGH");
        assert_eq!(iphone15.state, SimDeviceState::Shutdown);
        assert!(iphone15.is_available);
        assert!(iphone15.runtime_id.as_ref().unwrap().contains("iOS-17-0"));
    }

    #[test]
    fn parse_booted_state() {
        let devices = parse_simctl_devices(SIMCTL_DEVICES_JSON).unwrap();
        let pro = devices.iter().find(|d| d.name == "iPhone 15 Pro").unwrap();
        assert_eq!(pro.state, SimDeviceState::Booted);
    }

    #[test]
    fn parse_unavailable_device() {
        let devices = parse_simctl_devices(SIMCTL_DEVICES_JSON).unwrap();
        let iphone14 = devices.iter().find(|d| d.name == "iPhone 14").unwrap();
        assert!(!iphone14.is_available);
    }

    #[test]
    fn parse_runtimes() {
        let runtimes = parse_simctl_runtimes(SIMCTL_RUNTIMES_JSON).unwrap();
        assert_eq!(runtimes.len(), 2);
        assert_eq!(runtimes[0].name, "iOS 17.0");
        assert_eq!(runtimes[0].version, "17.0");
        assert!(runtimes[0].is_available);
    }

    #[test]
    fn parse_empty_devices() {
        let devices = parse_simctl_devices(r#"{"devices": {}}"#).unwrap();
        assert!(devices.is_empty());
    }

    #[test]
    fn parse_invalid_json_returns_error() {
        let result = parse_simctl_devices("not json");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to parse"));
    }

    #[test]
    fn parse_unknown_device_state() {
        let json = r#"{
            "devices": {
                "runtime-1": [
                    {
                        "udid": "x",
                        "name": "Test",
                        "state": "SomeNewState",
                        "isAvailable": true
                    }
                ]
            }
        }"#;
        let devices = parse_simctl_devices(json).unwrap();
        assert_eq!(devices[0].state, SimDeviceState::Unknown);
    }

    #[test]
    fn unified_state_mapping_from_parsed() {
        let devices = parse_simctl_devices(SIMCTL_DEVICES_JSON).unwrap();
        let booted = devices.iter().find(|d| d.name == "iPhone 15 Pro").unwrap();
        assert_eq!(
            booted.to_unified_state(),
            deepagent_mobile_core::DeviceState::Ready
        );
        let shutdown = devices.iter().find(|d| d.name == "iPhone 15").unwrap();
        assert_eq!(
            shutdown.to_unified_state(),
            deepagent_mobile_core::DeviceState::Disconnected
        );
    }
}
