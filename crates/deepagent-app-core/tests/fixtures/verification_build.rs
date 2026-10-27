//! Windows-only real build fixture shared by ChatService and kernel-boundary tests.
//! Every process and file belongs to this TempDir; no external package is used.

use deepagent_builtins::{CommandExecutor, CommandShell, SystemExecutor};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
};

pub struct BuildFixture {
    pub root: PathBuf,
    _temp: tempfile::TempDir,
    cold: bool,
}

impl BuildFixture {
    pub async fn new() -> Self {
        Self::prepare(false).await
    }

    pub async fn cold() -> Self {
        Self::prepare(true).await
    }

    async fn prepare(cold: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace 空格 $literal 'quote");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = 'verification-lifecycle-fixture'\nversion = '0.0.0'\nedition = '2021'\n[workspace]\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n").unwrap();
        std::fs::write(
            root.join("build.rs"),
            r#"use std::{fs, path::Path, process::Command, thread, time::{Duration, Instant}};
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--compiler-worker") {
        fs::write("compiler-worker-started", std::process::id().to_string()).unwrap();
        let began = Instant::now();
        while !Path::new("release-compiler").exists() {
            assert!(began.elapsed() < Duration::from_secs(15), "compiler fixture watchdog");
            thread::sleep(Duration::from_millis(10));
        }
        fs::write("compiler-wrote-after-release", "compiler gate released").unwrap();
        return;
    }
    // The same source can be compiled as Cargo's native rustc-wrapper. Forward
    // query invocations immediately; gate only the real build-script compile.
    if args.first().is_some_and(|arg| arg != "--fixture-worker") {
        let began = Instant::now();
        let stage = format!("compiler-wrapper-{}.stage", std::process::id());
        let compiling = args.windows(2).any(|pair| pair[0] == "--crate-name" && pair[1] == "build_script_build");
        let kind = if compiling { "build-script" } else { "query" };
        // Fixture-only progress, without command arguments or host paths.
        fs::write(&stage, format!("{kind} entered")).unwrap();
        if compiling {
            fs::write("compiler-started", std::process::id().to_string()).unwrap();
            let status = Command::new(std::env::current_exe().unwrap())
                .arg("--compiler-worker").status().unwrap();
            assert!(status.success());
        }
        fs::write(&stage, format!("{kind} forwarding elapsed_ms={}", began.elapsed().as_millis())).unwrap();
        let status = Command::new(&args[0]).args(&args[1..]).status().unwrap();
        fs::write(&stage, format!("{kind} completed exit={:?} elapsed_ms={}", status.code(), began.elapsed().as_millis())).unwrap();
        std::process::exit(status.code().unwrap_or(1));
    }
    if std::env::args().nth(1).as_deref() == Some("--fixture-worker") {
        fs::write("worker-started", std::process::id().to_string()).unwrap();
        let began = Instant::now();
        while !Path::new("release-build").exists() {
            assert!(began.elapsed() < Duration::from_secs(15), "fixture watchdog");
            thread::sleep(Duration::from_millis(10));
        }
        fs::write("build-wrote-after-release", "real descendant side effect").unwrap();
    } else {
        println!("cargo:rerun-if-changed=build.rs");
        println!("cargo:rerun-if-env-changed=DEEPAGENT_H64_FIXTURE_PREPARE");
        if std::env::var_os("DEEPAGENT_H64_FIXTURE_PREPARE").is_some() { return; }
        fs::write("build-started", std::process::id().to_string()).unwrap();
        let mut worker = Command::new(std::env::current_exe().unwrap())
            .arg("--fixture-worker").spawn().unwrap();
        // The parent-first case deliberately leaves inherited output pipes open.
        loop {
            if let Some(status) = worker.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Path::new("return-parent").exists() { break; }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
"#,
        )
        .unwrap();
        let fixture = Self {
            root,
            _temp: temp,
            cold,
        };
        if cold {
            // Compile the test helper, NOT this project's Cargo target. The
            // production Cargo must still compile and run build.rs from cold.
            let helper = SystemExecutor
                .run(
                    "rustc build.rs --edition=2021 --crate-name verification_compiler_wrapper -o ../compiler-wrapper.exe",
                    fixture.root.to_str().unwrap(),
                )
                .await
                .expect("compile native fixture wrapper");
            assert_eq!(helper.exit_code, Some(0), "{helper:?}");
            let wrapper = fixture.root.parent().unwrap().join("compiler-wrapper.exe");
            assert!(wrapper.is_file());
            assert!(!fixture.root.join("target").exists());
            assert!(std::env::var_os("RUSTC_WRAPPER").is_none());
            std::fs::create_dir(fixture.root.join(".cargo")).unwrap();
            std::fs::write(
                fixture.root.join(".cargo/config.toml"),
                format!(
                    "[build]\nrustc-wrapper = \"{}\"\n",
                    wrapper.to_str().unwrap().replace('\\', "/")
                ),
            )
            .unwrap();
            return fixture;
        }
        // Compiler/linker preparation is not the lifecycle under test. Keep it
        // outside the run's deadline and the unchanged 4 s readiness window.
        // Only this child receives the preparation variable: the production
        // Cargo invocation sees it change and must execute the real build.rs.
        assert!(std::env::var_os("DEEPAGENT_H64_FIXTURE_PREPARE").is_none());
        let began = Instant::now();
        let prepared = SystemExecutor
            .run_with_environment(
                "cargo check --offline --quiet",
                fixture.root.to_str().unwrap(),
                CommandShell::Auto,
                &[("DEEPAGENT_H64_FIXTURE_PREPARE".into(), "1".into())],
            )
            .await
            .expect("fixture compiler preparation must succeed");
        assert_eq!(prepared.exit_code, Some(0), "{prepared:?}");
        assert!(!fixture.root.join("build-started").exists());
        assert!(!fixture.root.join("worker-started").exists());
        assert!(!fixture.wrote());
        eprintln!(
            "verification-build-prepared elapsed_ms={}",
            began.elapsed().as_millis()
        );
        fixture
    }

    pub fn wrote(&self) -> bool {
        self.root.join("build-wrote-after-release").exists()
    }

    pub fn release(&self) {
        std::fs::write(self.root.join("release-build"), "release").unwrap();
        std::fs::write(self.root.join("release-compiler"), "release").unwrap();
    }

    pub fn assert_compiler_effects(&self, stopped: bool) {
        if self.cold {
            assert_eq!(
                self.root.join("compiler-wrote-after-release").exists(),
                !stopped
            );
            assert_eq!(self.root.join("build-started").exists(), !stopped);
            assert_eq!(self.root.join("worker-started").exists(), !stopped);
        }
    }

    pub async fn return_parent(&self, processes: &Processes) {
        std::fs::write(self.root.join("return-parent"), "return").unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !processes.parent_exited() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual build parent must exit before stopping the run");
        assert!(
            !processes.exited(),
            "worker must remain alive with inherited pipes"
        );
    }

    pub async fn try_started(&self) -> Result<Processes, String> {
        let began = Instant::now();
        let mut handles = Vec::new();
        let markers = if self.cold {
            ["compiler-started", "compiler-worker-started"]
        } else {
            ["build-started", "worker-started"]
        };
        for name in markers {
            let mut last_observation = String::new();
            let pid = tokio::time::timeout(Duration::from_secs(4), async {
                loop {
                    match std::fs::read_to_string(self.root.join(name)) {
                        Ok(text) => match text.parse::<u32>() {
                            Ok(pid) => break pid,
                            Err(error) => {
                                last_observation = format!("invalid fixture PID: {error}");
                            }
                        },
                        Err(error) => {
                            last_observation = format!("read {name}: {error}");
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .map_err(|_| {
                format!(
                    "real build must reach {name}; elapsed_ms={} last_observation={last_observation}; {}",
                    began.elapsed().as_millis(),
                    self.build_stage(),
                )
            })?;
            // PROCESS_SYNCHRONIZE only: no mutation or PID-reuse-based cleanup.
            let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
            assert!(!handle.is_null(), "{}", std::io::Error::last_os_error());
            // SAFETY: a fresh owned process handle returned by OpenProcess.
            handles.push(unsafe { OwnedHandle::from_raw_handle(handle.cast()) });
        }
        let processes = Processes(handles);
        assert!(!processes.exited());
        assert!(!self.wrote());
        if self.cold {
            assert!(!self.root.join("compiler-wrote-after-release").exists());
            assert!(!self.root.join("build-started").exists());
        }
        eprintln!(
            "verification-build-ready cold={} elapsed_ms={}",
            self.cold,
            began.elapsed().as_millis()
        );
        Ok(processes)
    }

    fn build_stage(&self) -> String {
        // Fixture files only: distinguish missing/partial PID markers from
        // compiler preparation without enumerating processes or host data.
        let build_root = self.root.join("target/debug/build");
        let mut files = Vec::new();
        match std::fs::read_dir(&build_root) {
            Ok(entries) => {
                for entry in entries {
                    match entry.and_then(|entry| std::fs::read_dir(entry.path())) {
                        Ok(children) => {
                            for child in children {
                                match child {
                                    Ok(child) => files.push(
                                        child
                                            .path()
                                            .strip_prefix(&self.root)
                                            .unwrap()
                                            .display()
                                            .to_string(),
                                    ),
                                    Err(error) => files.push(format!("read child: {error}")),
                                }
                            }
                        }
                        Err(error) => files.push(format!("read build directory: {error}")),
                    }
                }
            }
            Err(error) => files.push(format!("read target/debug/build: {error}")),
        }
        files.sort();
        let mut compiler_stages = Vec::new();
        match std::fs::read_dir(&self.root) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) => {
                            let name = entry.file_name();
                            let name = name.to_string_lossy();
                            if name.starts_with("compiler-wrapper-") && name.ends_with(".stage") {
                                compiler_stages.push(format!(
                                    "{name}: {:?}",
                                    std::fs::read_to_string(entry.path())
                                ));
                            }
                        }
                        Err(error) => compiler_stages.push(format!("read stage entry: {error}")),
                    }
                }
            }
            Err(error) => compiler_stages.push(format!("read stage directory: {error}")),
        }
        compiler_stages.sort();
        format!(
            "cargo_lock={} target_files={files:?} compiler_stages={compiler_stages:?} build_marker={} worker_marker={} canary={}",
            self.root.join("Cargo.lock").is_file(),
            self.root.join("build-started").is_file(),
            self.root.join("worker-started").is_file(),
            self.wrote(),
        )
    }
}

impl Drop for BuildFixture {
    fn drop(&mut self) {
        // Unblock before TempDir cleanup even if a test assertion unwinds.
        let _ = std::fs::write(self.root.join("release-build"), "release");
        let _ = std::fs::write(self.root.join("release-compiler"), "release");
    }
}

pub struct Processes(Vec<OwnedHandle>);

impl Processes {
    fn signaled(handle: &OwnedHandle) -> bool {
        // SAFETY: observation only; OwnedHandle lives through this call.
        let status = unsafe { WaitForSingleObject(handle.as_raw_handle().cast(), 0) };
        assert!(
            matches!(status, WAIT_OBJECT_0 | WAIT_TIMEOUT),
            "unexpected native wait {status}"
        );
        status == WAIT_OBJECT_0
    }

    fn parent_exited(&self) -> bool {
        Self::signaled(&self.0[0])
    }

    pub fn exited(&self) -> bool {
        self.0.iter().all(Self::signaled)
    }

    pub async fn settle(&self) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !self.exited() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("both real build and descendant must signal exit");
    }
}
