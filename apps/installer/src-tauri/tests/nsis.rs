#![cfg(windows)]
use deepagent_installer::{
    engine::run_nsis,
    payload::{hash_file, FileEntry, Manifest},
    CLI_EXE, MAIN_EXE, OPTIONS_EXE,
};
use std::{fs, os::windows::process::CommandExt, path::PathBuf, process::Command};

#[test]
#[ignore = "requires the NSIS compiler; explicitly run with --ignored"]
fn real_nsis_installs_to_unicode_space_path_and_rejects_failure_or_missing_files() {
    let compiler = std::env::var_os("DEEPAGENT_NSIS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("tauri/NSIS/makensis.exe")
        });
    assert!(
        compiler.exists(),
        "makensis unavailable (set DEEPAGENT_NSIS)"
    );
    // Everything stays inside one owned temporary directory. No product registry,
    // shortcuts, user data, installed program or original uninstall hook is used.
    let temp = tempfile::Builder::new()
        .prefix("deepagent-nsis-fixture-")
        .tempdir()
        .unwrap();
    // Also exercise the real additive hook and its uninstall callback without
    // ever executing the original data-cleanup macro or changing user HKCU.
    let hook = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("desktop")
        .join("src-tauri")
        .join("installer")
        .join("bootstrapper-hooks.nsh");
    let stub_helper = temp.path().join("fixture-helper.exe");
    let helper_source = temp.path().join("fixture.rs");
    fs::write(
        &helper_source,
        r##"#![windows_subsystem = "windows"]
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args[1], "uninstall");
    std::fs::write(std::path::Path::new(&args[2]).join("cleanup-called"), b"fixture").unwrap();
}"##,
    )
    .unwrap();
    let helper_build = Command::new(std::env::var_os("RUSTC").unwrap_or("rustc".into()))
        .arg(&helper_source)
        .arg("-o")
        .arg(&stub_helper)
        .output()
        .unwrap();
    assert!(
        helper_build.status.success(),
        "{}",
        String::from_utf8_lossy(&helper_build.stderr)
    );
    let hook_script = temp.path().join("hook.nsi");
    fs::write(&hook_script, format!("Unicode true\nRequestExecutionLevel user\n!include LogicLib.nsh\n!include FileFunc.nsh\n!define DEEPAGENT_OPTIONS_HELPER \"{}\"\n!include \"{}\"\nOutFile \"{}\"\nSection\n!insertmacro NSIS_HOOK_POSTINSTALL\nWriteUninstaller $INSTDIR\\uninstall.exe\nSectionEnd\nSection Uninstall\nSectionEnd\n", stub_helper.display(), hook.display(), temp.path().join("hook.exe").display())).unwrap();
    let compiled = Command::new(&compiler)
        .arg("/V2")
        .arg(&hook_script)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "hook compile: {} {}",
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );
    for update in [false, true] {
        let root = temp.path().join(if update {
            "upgrade folder 深海"
        } else {
            "uninstall folder 深海"
        });
        let install =
            deepagent_installer::platform::nsis_command(&temp.path().join("hook.exe"), &root)
                .status()
                .unwrap();
        assert!(install.success());
        let mut uninstaller = Command::new(root.join("uninstall.exe"));
        uninstaller.arg("/S");
        if update {
            uninstaller.arg("/UPDATE");
        }
        uninstaller.raw_arg(format!("_?={}", root.display()));
        assert!(uninstaller.status().unwrap().success());
        assert_eq!(
            root.join("cleanup-called").exists(),
            !update,
            "upgrade must preserve preferences"
        );
        assert!(
            !root.join(OPTIONS_EXE).exists(),
            "helper is removed after successful cleanup"
        );
    }
    let file = temp.path().join("payload.txt");
    fs::write(&file, b"isolated NSIS fixture").unwrap();
    let files: Vec<_> = [MAIN_EXE, CLI_EXE, OPTIONS_EXE]
        .into_iter()
        .map(|name| FileEntry {
            path: name.into(),
            bytes: fs::metadata(&file).unwrap().len(),
            sha256: hash_file(&file).unwrap(),
        })
        .collect();
    let meta = Manifest {
        version: "0.0.8".into(),
        setup_sha256: "a".repeat(64),
        installed_bytes: files.iter().map(|f| f.bytes).sum(),
        files,
    };
    for variant in ["success", "failure", "missing"] {
        let setup = temp.path().join(format!("{variant}.exe"));
        let script = temp.path().join(format!("{variant}.nsi"));
        let body = if variant == "failure" {
            "SetErrorLevel 7\nQuit".into()
        } else {
            [MAIN_EXE, CLI_EXE, OPTIONS_EXE]
                .into_iter()
                .take(if variant == "missing" { 1 } else { 3 })
                .map(|name| format!("File /oname={name} \"{}\"", file.display()))
                .collect::<Vec<_>>()
                .join("\n")
        };
        fs::write(&script, format!("Unicode true\nRequestExecutionLevel user\nOutFile \"{}\"\nSection\nSetOutPath $INSTDIR\n{body}\nSectionEnd\n", setup.display())).unwrap();
        let output = Command::new(&compiler)
            .arg("/V2")
            .arg(&script)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let root = temp.path().join(format!("{variant} folder 深海"));
        let mut phases = Vec::new();
        let result = run_nsis(&setup, &meta, &root, |p| {
            assert!(p.percent < 100);
            phases.push(p.phase);
        });
        assert_eq!(
            result.is_ok(),
            variant == "success",
            "{variant}: {result:?}"
        );
        if variant == "success" {
            assert!(root.join(MAIN_EXE).exists());
        }
        if variant == "failure" {
            assert!(!phases.contains(&"verifying"));
        }
    }
}
