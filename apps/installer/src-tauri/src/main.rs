#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
fn main() {
    #[cfg(windows)]
    if let Err(error) = deepagent_installer::ui::run() {
        use windows::{
            core::PCWSTR,
            Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
        };
        let message = deepagent_installer::platform::wide(error);
        let title = deepagent_installer::platform::wide("DeepAgent Studio 安装程序");
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(message.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONERROR,
            );
        }
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    {
        eprintln!("The DeepAgent Studio bootstrapper supports Windows x64 only.");
    }
}
