use crate::{payload::Manifest, platform, Progress};
use std::{path::Path, time::Duration};

/// The same process/wait/verification path is exercised with a real NSIS fixture.
/// The caller owns the extracted payload directory until this returns.
pub fn run_nsis(
    setup: &Path,
    meta: &Manifest,
    root: &Path,
    mut report: impl FnMut(Progress),
) -> Result<(), String> {
    let mut child = platform::nsis_command(setup, root)
        .spawn()
        .map_err(|e| format!("启动 NSIS 安装失败：{e}"))?;
    loop {
        match child
            .try_wait()
            .map_err(|e| format!("等待 NSIS 安装失败：{e}"))?
        {
            Some(status) => {
                if !status.success() {
                    return Err(format!(
                        "NSIS 安装未成功（退出码 {:?}），未标记安装完成",
                        status.code()
                    ));
                }
                break;
            }
            None => {
                let written = meta.observe(root);
                report(Progress {
                    phase: "installing",
                    percent: crate::observed_percent(written, meta.installed_bytes),
                    written_bytes: written,
                    total_bytes: meta.installed_bytes,
                });
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    report(Progress {
        phase: "verifying",
        percent: 96,
        written_bytes: meta.installed_bytes,
        total_bytes: meta.installed_bytes,
    });
    meta.verify_installation(root)
}
