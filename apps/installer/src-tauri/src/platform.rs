use crate::options::{Registry, Value};
use std::{
    ffi::OsStr,
    fs,
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::{Component, Path, PathBuf},
    process::Command,
};
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        Foundation::{LPARAM, WPARAM},
        Storage::FileSystem::GetDiskFreeSpaceExW,
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
        },
        UI::{
            Shell::{
                FOLDERID_Desktop, FOLDERID_LocalAppData, FOLDERID_Programs, IShellLinkW,
                SHGetKnownFolderPath, ShellLink, KNOWN_FOLDER_FLAG,
            },
            WindowsAndMessaging::{
                SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
            },
        },
    },
};
use winreg::{
    enums::*,
    types::{FromRegValue, ToRegValue},
    RegKey,
};

pub fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

pub struct UserRegistry;
impl Registry for UserRegistry {
    fn read(&self, key: &str, name: &str) -> Result<Option<Value>, String> {
        let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey(key) {
            Ok(key) => key,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("读取用户注册表失败：{e}")),
        };
        match key.get_raw_value(name) {
            Ok(raw) if raw.vtype == REG_SZ || raw.vtype == REG_EXPAND_SZ => Ok(Some(Value {
                text: String::from_reg_value(&raw).map_err(|e| e.to_string())?,
                expandable: raw.vtype == REG_EXPAND_SZ,
            })),
            Ok(_) => Err("用户注册表值类型异常，未覆盖".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn write(&self, key: &str, name: &str, value: &Value) -> Result<(), String> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
            .create_subkey(key)
            .map_err(|e| e.to_string())?;
        let mut raw = value.text.to_reg_value();
        raw.vtype = if value.expandable {
            REG_EXPAND_SZ
        } else {
            REG_SZ
        };
        key.set_raw_value(name, &raw)
            .map_err(|e| format!("写入用户注册表失败：{e}"))
    }
    fn delete(&self, key: &str, name: &str) -> Result<(), String> {
        let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(key, KEY_SET_VALUE)
        {
            Ok(key) => key,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        match key.delete_value(name) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

pub fn broadcast_environment() {
    let text = wide("Environment");
    // Other processes refresh their environment; no system PATH is written.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(text.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            1000,
            None,
        );
    }
}

pub fn webview_version() -> Option<String> {
    const CLIENT: &str =
        "Software\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    [
        RegKey::predef(HKEY_CURRENT_USER).open_subkey(CLIENT),
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(CLIENT, KEY_READ | KEY_WOW64_32KEY),
    ]
    .into_iter()
    .filter_map(Result::ok)
    .filter_map(|k| k.get_value::<String, _>("pv").ok())
    .find(|v| valid_webview_version(v))
}

fn valid_webview_version(version: &str) -> bool {
    let parts: Result<Vec<u32>, _> = version.split('.').map(str::parse).collect();
    parts.is_ok_and(|parts| parts.len() == 4 && parts.iter().any(|n| *n > 0))
}

#[derive(Clone, serde::Serialize)]
pub struct InstalledVersion {
    pub version: String,
    pub directory: String,
    pub machine: bool,
}

pub fn installed_version() -> Result<Option<InstalledVersion>, String> {
    let key = format!(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{}",
        crate::PRODUCT
    );
    for (hive, machine) in [(HKEY_CURRENT_USER, false), (HKEY_LOCAL_MACHINE, true)] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            match RegKey::predef(hive).open_subkey_with_flags(&key, KEY_READ | view) {
                Ok(key) => {
                    return Ok(Some(InstalledVersion {
                        version: key
                            .get_value("DisplayVersion")
                            .map_err(|e| format!("已装版本记录无效：{e}"))?,
                        directory: key
                            .get_value::<String, _>("InstallLocation")
                            .map_err(|e| format!("已装位置记录无效：{e}"))?
                            .trim_matches('"')
                            .into(),
                        machine,
                    }))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("读取已装版本失败：{e}")),
            }
        }
    }
    Ok(None)
}

fn known_folder(id: &windows::core::GUID) -> Result<PathBuf, String> {
    unsafe {
        let value =
            SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).map_err(|e| e.to_string())?;
        let text = value.to_string().map_err(|e| e.to_string());
        CoTaskMemFree(Some(value.as_ptr().cast()));
        text.map(PathBuf::from)
    }
}

pub fn default_directory() -> Result<PathBuf, String> {
    Ok(known_folder(&FOLDERID_LocalAppData)?.join(crate::PRODUCT))
}

pub fn destination(text: &str) -> Result<PathBuf, String> {
    let root = PathBuf::from(text);
    let mut parts = root.components();
    let drive = matches!(parts.next(), Some(Component::Prefix(p)) if matches!(p.kind(), std::path::Prefix::Disk(_)));
    if !drive
        || !root.is_absolute()
        || text.contains(['"', ';', '\r', '\n', '\0', '%'])
        || text.get(2..).is_none_or(|rest| rest.contains(':'))
        || text.encode_utf16().count() > 200
        || root.components().count() < 3
        || root
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || text
            .split(['\\', '/'])
            .skip(1)
            .any(|p| p.ends_with(['.', ' ']))
    {
        return Err(
            "请选择本地磁盘中的独立安装文件夹（不能是盘符根目录、网络路径或含特殊字符）".into(),
        );
    }
    crate::payload::reject_links(&root)?;
    Ok(root.components().collect())
}

pub fn available_space(root: &Path) -> Result<u64, String> {
    let ancestor = root
        .ancestors()
        .find(|p| p.exists())
        .ok_or("安装磁盘不存在")?;
    let mut bytes = 0;
    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(wide(ancestor).as_ptr()),
            Some(&mut bytes),
            None,
            None,
        )
    }
    .map_err(|e| format!("磁盘空间查询失败：{e}"))?;
    Ok(bytes)
}

pub fn check_writable(root: &Path) -> Result<(), String> {
    let parent = root
        .ancestors()
        .find(|p| p.is_dir())
        .ok_or("没有可写的父目录")?;
    tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("安装位置不可写，请使用当前用户目录：{e}"))?;
    Ok(())
}

/// NSIS /D is deliberately unquoted and last, unlike normal Windows arguments.
pub fn nsis_command(exe: &Path, root: &Path) -> Command {
    let mut command = Command::new(exe);
    command
        .arg("/S")
        .arg("/NS")
        .raw_arg(format!("/D={}", root.display()));
    command
}

pub fn shortcuts(root: &Path, desktop: bool, menu: bool) -> Result<(), String> {
    let root = root.to_path_buf();
    // A fresh STA thread avoids reusing a pool thread initialized by another
    // native dialog in a different COM apartment.
    std::thread::spawn(move || {
        let paths = [(FOLDERID_Desktop, desktop), (FOLDERID_Programs, menu)]
            .into_iter()
            .map(|(folder, enabled)| {
                Ok((
                    known_folder(&folder)?.join(format!("{}.lnk", crate::PRODUCT)),
                    enabled,
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        shortcuts_at(&root, paths)
    })
    .join()
    .map_err(|_| "快捷方式线程意外退出".to_string())?
}

fn shortcuts_at(root: &Path, paths: Vec<(PathBuf, bool)>) -> Result<(), String> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let _apartment = Apartment;
        for (path, enabled) in paths {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| e.to_string())?;
            let persist: IPersistFile = link.cast().map_err(|e| e.to_string())?;
            if path.exists() {
                persist
                    .Load(PCWSTR(wide(&path).as_ptr()), STGM_READ)
                    .map_err(|e| e.to_string())?;
                let mut target = [0_u16; 1024];
                link.GetPath(&mut target, std::ptr::null_mut(), 0)
                    .map_err(|e| e.to_string())?;
                let n = target.iter().position(|c| *c == 0).unwrap_or(target.len());
                if !String::from_utf16_lossy(&target[..n])
                    .eq_ignore_ascii_case(&root.join(crate::MAIN_EXE).to_string_lossy())
                {
                    return Err("同名快捷方式属于其他安装，未覆盖".into());
                }
                if !enabled {
                    fs::remove_file(&path).map_err(|e| e.to_string())?;
                }
            }
            if enabled {
                link.SetPath(PCWSTR(wide(root.join(crate::MAIN_EXE)).as_ptr()))
                    .map_err(|e| e.to_string())?;
                link.SetWorkingDirectory(PCWSTR(wide(root).as_ptr()))
                    .map_err(|e| e.to_string())?;
                persist
                    .Save(PCWSTR(wide(&path).as_ptr()), true)
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_shell_shortcuts_preserve_foreign_targets_and_remove_only_our_link() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Deep Agent 深海");
        fs::create_dir(&root).unwrap();
        fs::write(root.join(crate::MAIN_EXE), b"not executed").unwrap();
        let link = temp.path().join("fixture.lnk");
        shortcuts_at(&root, vec![(link.clone(), true)]).unwrap();
        assert!(link.exists());
        assert!(shortcuts_at(&temp.path().join("other"), vec![(link.clone(), false)]).is_err());
        assert!(link.exists());
        shortcuts_at(&root, vec![(link.clone(), false)]).unwrap();
        assert!(!link.exists());
    }
    #[test]
    fn missing_zero_and_malformed_runtime_versions_are_not_installed() {
        for value in ["", "0.0.0.0", "abc.1", "1.2.3", "1.2.3.4.5"] {
            assert!(!valid_webview_version(value), "{value}");
        }
        assert!(valid_webview_version("151.0.4129.78"));
    }
    #[test]
    fn rejects_dangerous_nsis_destinations() {
        for value in [
            "C:\\",
            "relative",
            "\\\\server\\share",
            "C:\\a\\..\\b",
            "C:\\bad;path",
            "C:\\bad\"path",
            "C:\\foo.\\bar",
            "C:\\%TEMP%\\app",
        ] {
            assert!(destination(value).is_err(), "{value}");
        }
        assert!(destination("G:\\installer test\\深海").is_ok());
    }
    #[test]
    fn nsis_destination_remains_unquoted_and_last() {
        let command = nsis_command(
            Path::new("G:\\payload\\setup.exe"),
            Path::new("G:\\test folder\\深海"),
        );
        let debug = format!("{command:?}");
        assert!(debug.contains("\"/S\" \"/NS\" /D=G:"), "{debug}");
        assert!(!debug.contains("/R"));
    }
}
