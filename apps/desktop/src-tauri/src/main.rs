// Prevent a console window on Windows for both debug and release installers.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    deepagent_desktop_lib::run();
}
