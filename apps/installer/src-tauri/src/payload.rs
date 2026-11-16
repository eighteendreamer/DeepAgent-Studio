use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Component, Path},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: String,
    pub setup_sha256: String,
    pub installed_bytes: u64,
    pub files: Vec<FileEntry>,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let manifest: Self =
            serde_json::from_str(text).map_err(|e| format!("安装清单无效：{e}"))?;
        let mut seen = std::collections::HashSet::new();
        for file in &manifest.files {
            let path = Path::new(&file.path);
            if path.is_absolute()
                || file.path.contains([':', '\\'])
                || file
                    .path
                    .split('/')
                    .any(|part| matches!(part, "" | "." | ".."))
                || path
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
                || !seen.insert(file.path.to_lowercase())
                || file.sha256.len() != 64
                || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("安装清单含越界、重复路径或无效摘要".into());
            }
        }
        let total = manifest
            .files
            .iter()
            .try_fold(0_u64, |n, f| n.checked_add(f.bytes));
        if total != Some(manifest.installed_bytes)
            || total == Some(0)
            || !seen.contains(crate::MAIN_EXE)
            || !seen.contains(crate::CLI_EXE)
            || !seen.contains(crate::OPTIONS_EXE)
        {
            return Err("安装清单缺少必需文件或大小不匹配".into());
        }
        Ok(manifest)
    }

    pub fn observe(&self, root: &Path) -> u64 {
        self.files
            .iter()
            .filter_map(|f| {
                fs::symlink_metadata(root.join(&f.path))
                    .ok()
                    .filter(|m| m.is_file() && !m.file_type().is_symlink())
                    .map(|m| m.len().min(f.bytes))
            })
            .sum()
    }

    pub fn verify_installation(&self, root: &Path) -> Result<(), String> {
        for file in &self.files {
            let path = root.join(&file.path);
            crate::payload::reject_links(&path)?;
            let actual =
                fs::metadata(&path).map_err(|e| format!("安装文件缺失 {}：{e}", file.path))?;
            if actual.len() != file.bytes || hash_file(&path)? != file.sha256 {
                return Err(format!("安装文件校验失败：{}", file.path));
            }
        }
        Ok(())
    }
}

pub fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("读取校验文件失败：{e}"))?;
    let mut hash = Sha256::new();
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("校验文件失败：{e}"))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn reject_links(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if meta.file_attributes() & 0x400 != 0 {
                        return Err("安装路径不能穿过链接或重解析点".into());
                    }
                }
                if meta.file_type().is_symlink() {
                    return Err("安装路径不能穿过符号链接".into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("无法检查安装路径边界：{error}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Manifest) {
        let dir = tempfile::tempdir().unwrap();
        let files = [crate::MAIN_EXE, crate::CLI_EXE, crate::OPTIONS_EXE].map(|name| {
            fs::write(dir.path().join(name), b"fixture").unwrap();
            FileEntry {
                path: name.into(),
                bytes: 7,
                sha256: hash_file(&dir.path().join(name)).unwrap(),
            }
        });
        (
            dir,
            Manifest {
                version: "0.0.8".into(),
                setup_sha256: "a".repeat(64),
                installed_bytes: 21,
                files: files.into(),
            },
        )
    }
    #[test]
    fn verifies_real_files_and_rejects_corruption_or_missing_files() {
        let (dir, manifest) = fixture();
        assert!(Manifest::parse(&serde_json::to_string(&manifest).unwrap()).is_ok());
        manifest.verify_installation(dir.path()).unwrap();
        fs::write(dir.path().join(crate::MAIN_EXE), b"corrupt").unwrap();
        assert!(manifest.verify_installation(dir.path()).is_err());
        fs::remove_file(dir.path().join(crate::CLI_EXE)).unwrap();
        assert!(manifest.verify_installation(dir.path()).is_err());
    }
    #[test]
    fn untrusted_manifest_cannot_escape_or_alias_paths() {
        for bad in [
            "../outside",
            "C:/outside",
            "/outside",
            "folder/../../outside",
            "stream:ads",
            "folder\\file",
            crate::CLI_EXE,
        ] {
            let (_, mut manifest) = fixture();
            manifest.files[0].path = bad.into();
            assert!(
                Manifest::parse(&serde_json::to_string(&manifest).unwrap()).is_err(),
                "{bad}"
            );
        }
    }
}
