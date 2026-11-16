#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    {
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        let result = if args.len() == 2 && args[0] == "uninstall" {
            deepagent_installer::platform::destination(&args[1]).and_then(|_| {
                deepagent_installer::options::cleanup(
                    &deepagent_installer::platform::UserRegistry,
                    &args[1],
                )
            })
        } else {
            Err("expected uninstall <installation-directory>".into())
        };
        if let Err(error) = result {
            eprintln!("installer option cleanup failed: {error}");
            std::process::exit(1);
        }
        deepagent_installer::platform::broadcast_environment();
    }
    #[cfg(not(windows))]
    {
        eprintln!("Windows installer helper is unavailable on this platform");
        std::process::exit(1);
    }
}
