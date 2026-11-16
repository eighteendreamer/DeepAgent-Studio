use crate::{
    payload::{hash_file, Manifest},
    platform, InstallOptions, Progress, MAIN_EXE,
};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/payload.rs"));
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Installing,
    Options(InstallOptions),
    Ready(InstallOptions),
    Launching,
}
#[derive(Default)]
struct Installer {
    phase: Mutex<Phase>,
}
impl Installer {
    fn busy(&self) -> bool {
        self.phase
            .lock()
            .map_or(true, |p| matches!(*p, Phase::Installing | Phase::Launching))
    }
}

fn manifest() -> Result<Manifest, String> {
    if embedded::SETUP.is_empty() {
        return Err("开发模式未嵌入 NSIS 包；界面可预览，但不能执行安装".into());
    }
    Manifest::parse(embedded::MANIFEST)
}

fn extract_setup() -> Result<(tempfile::TempDir, PathBuf), String> {
    let meta = manifest()?;
    let dir = tempfile::Builder::new()
        .prefix("deepagent-setup-")
        .tempdir()
        .map_err(|e| format!("创建安装暂存目录失败：{e}"))?;
    if platform::available_space(dir.path())? < embedded::SETUP.len() as u64 + 64 * 1024 * 1024 {
        return Err("系统临时目录所在磁盘空间不足".into());
    }
    let exe = dir.path().join("setup.exe");
    fs::write(&exe, embedded::SETUP).map_err(|e| format!("释放 NSIS 安装包失败：{e}"))?;
    if hash_file(&exe)? != meta.setup_sha256 {
        return Err("内嵌安装包完整性校验失败".into());
    }
    Ok((dir, exe))
}

#[derive(Serialize)]
struct Environment {
    directory: String,
    available_bytes: u64,
    required_bytes: u64,
    webview_version: Option<String>,
    installed: Option<platform::InstalledVersion>,
    target_version: String,
    payload_available: bool,
}

#[tauri::command]
fn check_env(directory: Option<String>) -> Result<Environment, String> {
    let installed = platform::installed_version()?;
    let root = match directory {
        Some(path) => platform::destination(&path)?,
        None => platform::destination(
            &installed
                .as_ref()
                .filter(|i| !i.machine)
                .map(|i| i.directory.clone())
                .unwrap_or(
                    platform::default_directory()?
                        .to_string_lossy()
                        .into_owned(),
                ),
        )?,
    };
    let meta = if embedded::SETUP.is_empty() {
        None
    } else {
        Some(manifest()?)
    };
    Ok(Environment {
        directory: root.to_string_lossy().into_owned(),
        available_bytes: platform::available_space(&root)?,
        required_bytes: meta
            .as_ref()
            .map_or(0, |m| m.installed_bytes.saturating_add(64 * 1024 * 1024)),
        webview_version: platform::webview_version(),
        installed,
        target_version: meta
            .as_ref()
            .map_or(env!("CARGO_PKG_VERSION").into(), |m| m.version.clone()),
        payload_available: meta.is_some(),
    })
}

#[tauri::command]
async fn choose_directory(app: tauri::AppHandle) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("选择 DeepAgent Studio 安装文件夹")
            .blocking_pick_folder()
            .map(|p| {
                p.into_path()
                    .map(|p| p.join(crate::PRODUCT).to_string_lossy().into_owned())
                    .map_err(|e| e.to_string())
            })
            .transpose()
    })
    .await
    .map_err(|e| e.to_string())?
}

fn progress(app: &tauri::AppHandle, phase: &'static str, percent: u8, written: u64, total: u64) {
    if let Err(error) = app.emit(
        "install-progress",
        Progress {
            phase,
            percent,
            written_bytes: written,
            total_bytes: total,
        },
    ) {
        eprintln!("installer progress delivery failed: {error}");
    }
}

fn run_install(app: &tauri::AppHandle, options: &InstallOptions) -> Result<(), String> {
    let meta = manifest()?;
    let root = platform::destination(&options.directory)?;
    if let Some(previous) = platform::installed_version()? {
        if previous.machine {
            return Err("检测到全用户安装，请使用原安装包进行维护，不混用当前用户安装".into());
        }
        if !Path::new(&previous.directory).eq_ignore_ascii_case_path(&root) {
            return Err("已有安装位于其他目录；请使用原位置升级，或先卸载旧版本".into());
        }
    }
    if platform::available_space(&root)? < meta.installed_bytes.saturating_add(64 * 1024 * 1024) {
        return Err("安装磁盘空间不足".into());
    }
    platform::check_writable(&root)?;
    progress(app, "preparing", 0, 0, meta.installed_bytes);
    let (_temporary, setup) = extract_setup()?;
    crate::engine::run_nsis(&setup, &meta, &root, |value| {
        progress(
            app,
            value.phase,
            value.percent,
            value.written_bytes,
            value.total_bytes,
        )
    })?;
    progress(
        app,
        "options",
        98,
        meta.installed_bytes,
        meta.installed_bytes,
    );
    Ok(())
}

trait WindowsPathEq {
    fn eq_ignore_ascii_case_path(&self, other: &Path) -> bool;
}
impl WindowsPathEq for Path {
    fn eq_ignore_ascii_case_path(&self, other: &Path) -> bool {
        self.to_string_lossy()
            .trim_end_matches('\\')
            .replace('/', "\\")
            .eq_ignore_ascii_case(
                &other
                    .to_string_lossy()
                    .trim_end_matches('\\')
                    .replace('/', "\\"),
            )
    }
}

#[tauri::command]
async fn install(
    app: tauri::AppHandle,
    state: State<'_, Arc<Installer>>,
    mut options: InstallOptions,
) -> Result<(), String> {
    options.directory = platform::destination(&options.directory)?
        .to_string_lossy()
        .into_owned();
    let state = state.inner().clone();
    {
        let mut phase = state.phase.lock().map_err(|e| e.to_string())?;
        if !matches!(*phase, Phase::Idle) {
            return Err("安装已在进行或已完成，请勿重复执行".into());
        }
        *phase = Phase::Installing;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let result = run_install(&app, &options);
        let mut phase = state.phase.lock().map_err(|e| e.to_string())?;
        *phase = if result.is_ok() {
            Phase::Options(options)
        } else {
            Phase::Idle
        };
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn apply_options(
    app: tauri::AppHandle,
    state: State<'_, Arc<Installer>>,
) -> Result<(), String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Serialize option changes and exclude launch/close while they are in progress.
        let mut phase = state.phase.lock().map_err(|e| e.to_string())?;
        let Phase::Options(options) = &*phase else {
            return Err("NSIS 安装与文件校验尚未完成".into());
        };
        let options = options.clone();
        platform::shortcuts(
            Path::new(&options.directory),
            options.desktop_shortcut,
            options.start_menu_shortcut,
        )?;
        crate::options::apply(
            &platform::UserRegistry,
            &options.directory,
            options.add_to_path,
            options.autostart,
        )?;
        platform::broadcast_environment();
        *phase = Phase::Ready(options);
        let meta = manifest()?;
        progress(
            &app,
            "complete",
            100,
            meta.installed_bytes,
            meta.installed_bytes,
        );
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn launch_app(app: tauri::AppHandle, state: State<'_, Arc<Installer>>) -> Result<(), String> {
    let state = state.inner().clone();
    let options = {
        let mut phase = state.phase.lock().map_err(|e| e.to_string())?;
        let Phase::Ready(options) = &*phase else {
            return Err("安装尚未完成，不能启动主程序".into());
        };
        let options = options.clone();
        *phase = Phase::Launching;
        options
    };
    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| {
            let meta = manifest()?;
            let root = platform::destination(&options.directory)?;
            let exe = root.join(MAIN_EXE);
            crate::payload::reject_links(&exe)?;
            let expected = meta
                .files
                .iter()
                .find(|f| f.path == MAIN_EXE)
                .ok_or("主程序未列入安装清单")?;
            if hash_file(&exe)? != expected.sha256 {
                return Err("主程序已改变，未执行".into());
            }
            let mut child = Command::new(exe)
                .current_dir(&root)
                .spawn()
                .map_err(|e| format!("启动主程序失败：{e}"))?;
            std::thread::sleep(Duration::from_millis(250));
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                return Err(format!("主程序启动后立即退出：{status}"));
            }
            // The existing desktop splash owns the startup animation. Never show
            // a second simulated splash in the installer or close on spawn error.
            app.exit(0);
            Ok(())
        })();
        if result.is_err() {
            *state.phase.lock().map_err(|e| e.to_string())? = Phase::Ready(options);
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn close_installer(app: tauri::AppHandle, state: State<'_, Arc<Installer>>) -> Result<(), String> {
    if state.busy() {
        return Err("安装或启动尚在进行，暂时不能关闭".into());
    }
    app.exit(0);
    Ok(())
}

pub fn run() -> Result<(), String> {
    // Do not let "Run as administrator" turn a user installation and its
    // launched agent/terminal into an elevated process tree.
    if unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() } {
        return Err("请使用普通权限运行品牌安装程序。它只安装到当前用户，不需要“以管理员身份运行”。全用户安装请使用原 NSIS 包。".into());
    }
    // This branch runs before Builder/build creates any WebView.
    if platform::webview_version().is_none() {
        let (_temporary, setup) = extract_setup()?;
        let status = Command::new(setup)
            .status()
            .map_err(|e| format!("打开原生安装向导失败：{e}"))?;
        if !status.success() {
            return Err(format!("原生安装向导未成功完成：{status}"));
        }
        return Ok(());
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::new(Installer::default()))
        .invoke_handler(tauri::generate_handler![
            check_env,
            choose_directory,
            install,
            apply_options,
            launch_app,
            close_installer
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.state::<Arc<Installer>>().busy() {
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .map_err(|e| format!("安装界面启动失败：{e}"))
}
