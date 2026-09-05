//! Parser for emulator command output.
//!
//! Parses `emulator -list-avds` output into structured `AvdInfo` records.

use deepagent_mobile_protocol::AvdInfo;

/// Parse the output of `emulator -list-avds`.
///
/// Each line is an AVD name. Empty lines and lines starting with whitespace
/// are ignored.
pub fn parse_list_avds(output: &str) -> Vec<AvdInfo> {
    output
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(|name| AvdInfo {
            name: name.to_string(),
            path: None,
            target: None,
            running: false,
            serial: None,
        })
        .collect()
}

/// Parse `avdmanager list avd` output for richer AVD information.
///
/// The output format is:
/// ```text
/// Available Android Virtual Devices:
///     Name: Pixel_4
///     Device: pixel (Google)
///     Path: /home/user/.android/avd/Pixel_4.avd
///     Target: Google Play (Google Inc.)
///     Based on: Android 11.0 (R) Tag/ABI: google_apis_playstore/x86
/// ```
pub fn parse_avdmanager_list(output: &str) -> Vec<AvdInfo> {
    let mut avds = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_path: Option<String> = None;
    let mut current_target: Option<String> = None;

    for line in output.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("Name: ") {
            if let Some(prev_name) = current_name.take() {
                avds.push(AvdInfo {
                    name: prev_name,
                    path: current_path.take(),
                    target: current_target.take(),
                    running: false,
                    serial: None,
                });
            }
            current_name = Some(name.trim().to_string());
        } else if let Some(path) = line.strip_prefix("Path: ") {
            current_path = Some(path.trim().to_string());
        } else if let Some(target) = line.strip_prefix("Target: ") {
            current_target = Some(target.trim().to_string());
        }
    }

    if let Some(name) = current_name {
        avds.push(AvdInfo {
            name,
            path: current_path,
            target: current_target,
            running: false,
            serial: None,
        });
    }

    avds
}

/// Check if an emulator serial (e.g., "emulator-5554") corresponds to a
/// running AVD by matching against known emulator ports.
///
/// Emulator consoles use ports 5554, 5556, 5558, etc. The serial format is
/// "emulator-<port>".
pub fn extract_emulator_port(serial: &str) -> Option<u16> {
    serial
        .strip_prefix("emulator-")
        .and_then(|port_str| port_str.parse::<u16>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_avds_typical() {
        let output = "Pixel_4\nPixel_6\nPixel_7_API_33\n";
        let avds = parse_list_avds(output);
        assert_eq!(avds.len(), 3);
        assert_eq!(avds[0].name, "Pixel_4");
        assert_eq!(avds[1].name, "Pixel_6");
        assert_eq!(avds[2].name, "Pixel_7_API_33");
    }

    #[test]
    fn parse_list_avds_empty() {
        let avds = parse_list_avds("");
        assert!(avds.is_empty());
    }

    #[test]
    fn parse_list_avds_with_blank_lines() {
        let output = "\nPixel_4\n\nPixel_6\n\n";
        let avds = parse_list_avds(output);
        assert_eq!(avds.len(), 2);
    }

    #[test]
    fn parse_avdmanager_list_typical() {
        let output = r#"Available Android Virtual Devices:
    Name: Pixel_4
    Device: pixel (Google)
    Path: /home/user/.android/avd/Pixel_4.avd
    Target: Google Play (Google Inc.)
    Based on: Android 11.0 (R)
    Name: Pixel_6
    Device: pixel6 (Google)
    Path: /home/user/.android/avd/Pixel_6.avd
    Target: Android 13.0 (Tiramisu)
"#;
        let avds = parse_avdmanager_list(output);
        assert_eq!(avds.len(), 2);
        assert_eq!(avds[0].name, "Pixel_4");
        assert_eq!(
            avds[0].path.as_deref(),
            Some("/home/user/.android/avd/Pixel_4.avd")
        );
        assert_eq!(avds[0].target.as_deref(), Some("Google Play (Google Inc.)"));
        assert_eq!(avds[1].name, "Pixel_6");
    }

    #[test]
    fn parse_avdmanager_list_empty() {
        let avds = parse_avdmanager_list("Available Android Virtual Devices:\n");
        assert!(avds.is_empty());
    }

    #[test]
    fn extract_emulator_port_valid() {
        assert_eq!(extract_emulator_port("emulator-5554"), Some(5554));
        assert_eq!(extract_emulator_port("emulator-5556"), Some(5556));
    }

    #[test]
    fn extract_emulator_port_invalid() {
        assert_eq!(extract_emulator_port("192.168.1.1:5555"), None);
        assert_eq!(extract_emulator_port("emulator-abc"), None);
    }
}
