//! Ownership-aware user registry changes, also used by the NSIS cleanup helper.
use serde::{Deserialize, Serialize};

pub const RECORD_KEY: &str = "Software\\DeepAgent Studio\\InstallerOptions";
pub const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Value {
    pub text: String,
    pub expandable: bool,
}

/// Restricted to HKCU by the Windows implementation. No arbitrary hives/commands
/// are exposed over IPC.
pub trait Registry {
    fn read(&self, key: &str, name: &str) -> Result<Option<Value>, String>;
    fn write(&self, key: &str, name: &str, value: &Value) -> Result<(), String>;
    fn delete(&self, key: &str, name: &str) -> Result<(), String>;
}

#[derive(Default, Serialize, Deserialize)]
pub struct Record {
    pub directory: String,
    pub path_added: bool,
    pub run_added: bool,
}

fn same_path(a: &str, b: &str) -> bool {
    a.trim()
        .trim_matches('"')
        .trim_end_matches('\\')
        .eq_ignore_ascii_case(b.trim_end_matches('\\'))
}

fn read_record(reg: &impl Registry) -> Result<Option<Record>, String> {
    reg.read(RECORD_KEY, "Record")?
        .map(|v| {
            serde_json::from_str(&v.text).map_err(|e| format!("安装选项记录损坏，拒绝覆盖：{e}"))
        })
        .transpose()
}

fn save_record(reg: &impl Registry, record: &Record) -> Result<(), String> {
    reg.write(
        RECORD_KEY,
        "Record",
        &Value {
            text: serde_json::to_string(record).map_err(|e| e.to_string())?,
            expandable: false,
        },
    )
}

pub fn apply(reg: &impl Registry, dir: &str, path: bool, autostart: bool) -> Result<(), String> {
    let mut record = read_record(reg)?.unwrap_or_default();
    if !record.directory.is_empty() && !same_path(&record.directory, dir) {
        return Err("检测到另一安装位置的选项记录，请先卸载原安装".into());
    }
    record.directory = dir.into();
    // Write-ahead ownership record survives interruptions and partial failures.
    if path {
        let mut value = reg.read("Environment", "Path")?.unwrap_or(Value {
            text: String::new(),
            expandable: true,
        });
        if !value.text.split(';').any(|part| same_path(part, dir)) {
            record.path_added = true;
            save_record(reg, &record)?;
            if !value.text.is_empty() && !value.text.ends_with(';') {
                value.text.push(';');
            }
            value.text.push_str(dir);
            reg.write("Environment", "Path", &value)?;
        }
    } else if record.path_added {
        remove_path(reg, dir)?;
        record.path_added = false;
    }
    let command = format!("\"{dir}\\{}\"", crate::MAIN_EXE);
    if autostart {
        let current = reg.read(RUN_KEY, crate::PRODUCT)?;
        if current.as_ref().is_some_and(|v| v.text != command) {
            return Err("登录启动项已指向其他程序，未覆盖".into());
        }
        if current.is_none() || record.run_added {
            record.run_added = true;
            save_record(reg, &record)?;
            reg.write(
                RUN_KEY,
                crate::PRODUCT,
                &Value {
                    text: command,
                    expandable: false,
                },
            )?;
        }
    } else if record.run_added {
        remove_run(reg, &command)?;
        record.run_added = false;
    }
    save_record(reg, &record)
}

fn remove_path(reg: &impl Registry, dir: &str) -> Result<(), String> {
    if let Some(mut value) = reg.read("Environment", "Path")? {
        value.text = value
            .text
            .split(';')
            .filter(|part| !same_path(part, dir))
            .collect::<Vec<_>>()
            .join(";");
        reg.write("Environment", "Path", &value)?;
    }
    Ok(())
}

fn remove_run(reg: &impl Registry, command: &str) -> Result<(), String> {
    if reg
        .read(RUN_KEY, crate::PRODUCT)?
        .is_some_and(|v| v.text == command)
    {
        reg.delete(RUN_KEY, crate::PRODUCT)?;
    }
    Ok(())
}

pub fn cleanup(reg: &impl Registry, dir: &str) -> Result<(), String> {
    let Some(record) = read_record(reg)? else {
        return Ok(());
    };
    if !same_path(&record.directory, dir) {
        return Err("卸载路径与选项归属不匹配，未修改用户环境".into());
    }
    if record.path_added {
        remove_path(reg, dir)?;
    }
    if record.run_added {
        remove_run(
            reg,
            &format!("\"{}\\{}\"", record.directory, crate::MAIN_EXE),
        )?;
    }
    reg.delete(RECORD_KEY, "Record")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};
    #[derive(Default)]
    struct Memory(RefCell<HashMap<(String, String), Value>>);
    impl Registry for Memory {
        fn read(&self, k: &str, n: &str) -> Result<Option<Value>, String> {
            Ok(self.0.borrow().get(&(k.into(), n.into())).cloned())
        }
        fn write(&self, k: &str, n: &str, v: &Value) -> Result<(), String> {
            self.0.borrow_mut().insert((k.into(), n.into()), v.clone());
            Ok(())
        }
        fn delete(&self, k: &str, n: &str) -> Result<(), String> {
            self.0.borrow_mut().remove(&(k.into(), n.into()));
            Ok(())
        }
    }
    #[test]
    fn options_are_idempotent_and_cleanup_preserves_user_additions_and_value_type() {
        let reg = Memory::default();
        let initial = Value {
            text: "%USERPROFILE%\\bin;;C:\\other".into(),
            expandable: true,
        };
        reg.write("Environment", "Path", &initial).unwrap();
        apply(&reg, "C:\\Deep Agent", true, true).unwrap();
        apply(&reg, "C:\\Deep Agent", true, true).unwrap();
        let mut user = reg.read("Environment", "Path").unwrap().unwrap();
        user.text.push_str(";C:\\later");
        reg.write("Environment", "Path", &user).unwrap();
        cleanup(&reg, "c:\\deep agent").unwrap();
        assert_eq!(
            reg.read("Environment", "Path").unwrap().unwrap(),
            Value {
                text: format!("{};C:\\later", initial.text),
                expandable: true
            }
        );
        assert!(reg.read(RUN_KEY, crate::PRODUCT).unwrap().is_none());
    }
    #[test]
    fn preexisting_equivalent_path_and_startup_are_not_owned_or_removed() {
        let reg = Memory::default();
        let initial = Value {
            text: "\"c:\\Deep Agent\\\";C:\\other".into(),
            expandable: false,
        };
        reg.write("Environment", "Path", &initial).unwrap();
        apply(&reg, "C:\\Deep Agent", true, false).unwrap();
        cleanup(&reg, "C:\\Deep Agent").unwrap();
        assert_eq!(reg.read("Environment", "Path").unwrap().unwrap(), initial);
    }
    #[test]
    fn conflicting_or_changed_startup_and_wrong_root_are_never_deleted() {
        let reg = Memory::default();
        apply(&reg, "C:\\app", true, true).unwrap();
        assert!(cleanup(&reg, "C:\\someone-else").is_err());
        let user = Value {
            text: "C:\\another.exe".into(),
            expandable: false,
        };
        reg.write(RUN_KEY, crate::PRODUCT, &user).unwrap();
        assert!(apply(&reg, "C:\\app", true, true).is_err());
        cleanup(&reg, "C:\\app").unwrap();
        assert_eq!(reg.read(RUN_KEY, crate::PRODUCT).unwrap(), Some(user));
    }
    #[test]
    fn unchecking_only_removes_options_we_added() {
        let reg = Memory::default();
        apply(&reg, "C:\\app", true, true).unwrap();
        apply(&reg, "C:\\app", false, false).unwrap();
        assert_eq!(reg.read("Environment", "Path").unwrap().unwrap().text, "");
        assert!(reg.read(RUN_KEY, crate::PRODUCT).unwrap().is_none());
    }
}
