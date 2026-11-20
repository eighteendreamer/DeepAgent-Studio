//! Desktop-only pet assets and preferences, beneath the existing user data root.
//! The shipped catalog contains relative paths; no runtime dependency on reference sources.
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MIN_SIZE: u32 = 280;
pub const MAX_SIZE: u32 = 800;

pub const CATALOG_JSON: &str = include_str!("../resources/pets/dsh-pet/catalog.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub version: u32,
    pub pack_id: String,
    pub resource_version: String,
    pub characters: Vec<Character>,
    pub files: Vec<AssetFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Character {
    pub id: String,
    pub name: String,
    pub thumbnail: String,
    pub preview: Option<String>,
    pub canvas: Dimensions,
    pub hit_box: Rect,
    pub default_size: u32,
    pub actions: BTreeMap<String, Action>,
    pub states: BTreeMap<String, ActionState>,
    pub groups: Vec<ActionGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub file: String,
    pub preview: Option<String>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionState {
    pub actions: Vec<String>,
    pub r#loop: bool,
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionGroup {
    pub id: String,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    pub version: u32,
    pub enabled: bool,
    pub sleeping: bool,
    pub character_id: String,
    pub size: u32,
    pub always_on_top: bool,
    pub follow_work: bool,
    pub bubbles: bool,
    pub position: Option<Position>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: false,
            sleeping: false,
            character_id: "blue-maid".into(),
            size: 462,
            always_on_top: true,
            follow_work: true,
            bubbles: true,
            position: None,
        }
    }
}

impl Preferences {
    pub fn awake(&self) -> bool {
        self.enabled && !self.sleeping
    }

    pub fn validate(&self, catalog: &Catalog) -> Result<(), String> {
        if self.version != 1 || !(MIN_SIZE..=MAX_SIZE).contains(&self.size) {
            return Err(format!(
                "桌宠配置版本或尺寸无效（允许 {MIN_SIZE}–{MAX_SIZE}）"
            ));
        }
        if !catalog
            .characters
            .iter()
            .any(|pet| pet.id == self.character_id)
        {
            return Err("桌宠角色不存在".into());
        }
        Ok(())
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreferencesPatch {
    pub enabled: Option<bool>,
    pub sleeping: Option<bool>,
    pub character_id: Option<String>,
    pub size: Option<u32>,
    pub always_on_top: Option<bool>,
    pub follow_work: Option<bool>,
    pub bubbles: Option<bool>,
}

impl PreferencesPatch {
    pub fn apply(self, current: &Preferences) -> Preferences {
        let mut next = current.clone();
        if let Some(value) = self.enabled {
            next.enabled = value;
            if value {
                next.sleeping = false;
            }
        }
        if let Some(value) = self.sleeping {
            next.sleeping = value;
        }
        if let Some(value) = self.character_id {
            next.character_id = value;
        }
        if let Some(value) = self.size {
            next.size = value;
        }
        if let Some(value) = self.always_on_top {
            next.always_on_top = value;
        }
        if let Some(value) = self.follow_work {
            next.follow_work = value;
        }
        if let Some(value) = self.bubbles {
            next.bubbles = value;
        }
        next
    }
}

pub struct PetStore {
    pub root: PathBuf,
    pub source: PathBuf,
    pub catalog: Catalog,
    preferences: Option<Preferences>,
    installed: bool,
}

impl PetStore {
    pub fn new(root: PathBuf, source: PathBuf) -> Self {
        Self {
            root,
            source,
            catalog: serde_json::from_str(CATALOG_JSON).expect("validated bundled pet catalog"),
            preferences: None,
            installed: false,
        }
    }

    pub fn resource_root(&self) -> PathBuf {
        self.root
            .join(&self.catalog.pack_id)
            .join(&self.catalog.resource_version)
    }

    pub fn preferences(&mut self) -> Result<Preferences, String> {
        if let Some(preferences) = &self.preferences {
            return Ok(preferences.clone());
        }
        let file = self.root.join("settings.json");
        let preferences = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice::<Preferences>(&bytes)
                .map_err(|error| format!("读取桌宠配置失败（{}）：{error}", file.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Preferences::default(),
            Err(error) => return Err(format!("读取桌宠配置失败：{error}")),
        };
        preferences.validate(&self.catalog)?;
        self.preferences = Some(preferences.clone());
        Ok(preferences)
    }

    pub fn save(&mut self, preferences: Preferences) -> Result<(), String> {
        preferences.validate(&self.catalog)?;
        ensure_directory(&self.root)?;
        let file = managed_path(&self.root, "settings.json")?;
        atomic_write(
            &file,
            &serde_json::to_vec_pretty(&preferences).map_err(|e| e.to_string())?,
        )?;
        self.preferences = Some(preferences);
        Ok(())
    }

    pub fn install(&mut self) -> Result<(), String> {
        if self.installed {
            return Ok(());
        }
        ensure_directory(&self.root)?;
        let relative = format!("{}/{}", self.catalog.pack_id, self.catalog.resource_version);
        let destination = managed_path(&self.root, &relative)?;
        ensure_directory(&destination)?;
        for asset in &self.catalog.files {
            let source = managed_path(&self.source, &asset.path)?;
            let target = managed_path(&destination, &asset.path)?;
            let metadata = std::fs::metadata(&source)
                .map_err(|error| format!("桌宠内置资源缺失（{}）：{error}", source.display()))?;
            if !metadata.is_file() || metadata.len() != asset.bytes {
                return Err(format!("桌宠内置资源大小不匹配：{}", asset.path));
            }
            if std::fs::metadata(&target).is_ok_and(|metadata| metadata.len() == asset.bytes) {
                continue;
            }
            if let Some(parent) = target.parent() {
                ensure_directory(parent)?;
            }
            let bytes = std::fs::read(&source).map_err(|error| error.to_string())?;
            atomic_write(&target, &bytes)?;
        }
        atomic_write(&destination.join("catalog.json"), CATALOG_JSON.as_bytes())?;
        self.installed = true;
        Ok(())
    }
}

fn ensure_directory(directory: &Path) -> Result<(), String> {
    reject_link(directory)?;
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("创建桌宠资源目录失败（{}）：{error}", directory.display()))
}

fn reject_link(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            let reparse_point = {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let reparse_point = false;
            if metadata.file_type().is_symlink() || reparse_point {
                return Err(format!(
                    "桌宠资源路径不允许链接或联接点：{}",
                    path.display()
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("检查桌宠资源路径失败：{error}")),
    }
}

pub fn managed_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty()
        || relative.contains(['\\', ':'])
        || !Path::new(relative)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err("桌宠资源必须使用目录内的相对路径".into());
    }
    reject_link(root)?;
    let mut path = root.to_path_buf();
    for part in Path::new(relative).components() {
        path.push(part);
        reject_link(&path)?;
    }
    Ok(path)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    reject_link(path)?;
    let parent = path.parent().ok_or("桌宠资源没有父目录")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temporary.write_all(bytes).map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary
        .persist(path)
        .map_err(|e| format!("保存桌宠资源失败：{e}"))?;
    Ok(())
}

/// Keep the whole transparent canvas visible, including on negative-coordinate monitors.
pub fn clamp_position(position: Position, width: f64, height: f64, areas: &[Rect]) -> Position {
    let Some(area) = areas.iter().min_by(|a, b| {
        let distance = |area: &Rect| {
            let x = f64::from(position.x).clamp(area.x, area.x + area.width);
            let y = f64::from(position.y).clamp(area.y, area.y + area.height);
            (x - f64::from(position.x)).powi(2) + (y - f64::from(position.y)).powi(2)
        };
        distance(a).total_cmp(&distance(b))
    }) else {
        return position;
    };
    Position {
        x: f64::from(position.x).clamp(area.x, area.x + (area.width - width).max(0.0)) as i32,
        y: f64::from(position.y).clamp(area.y, area.y + (area.height - height).max(0.0)) as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resources_install_to_the_user_root_and_repair_missing_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".deepagent/files/pets");
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pets/dsh-pet");
        let mut store = PetStore::new(root.clone(), source);
        store.install().unwrap();
        let installed = store.resource_root();
        assert!(installed.starts_with(root));
        assert!(installed.join("catalog.json").is_file());
        assert!(installed.join("character.png").is_file());
        for asset in &store.catalog.files {
            assert_eq!(
                std::fs::metadata(installed.join(&asset.path))
                    .unwrap()
                    .len(),
                asset.bytes
            );
        }
        let animation = installed.join("webm/待机呼吸休闲.webm");
        std::fs::write(&animation, b"truncated").unwrap();
        store.installed = false;
        store.install().unwrap();
        assert!(std::fs::metadata(animation).unwrap().len() > 1000);
    }

    #[test]
    fn preferences_round_trip_without_losing_position_when_controls_change() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = PetStore::new(temp.path().to_path_buf(), PathBuf::new());
        let mut original = store.preferences().unwrap();
        original.position = Some(Position { x: -640, y: 200 });
        store.save(original.clone()).unwrap();
        let changed = PreferencesPatch {
            size: Some(600),
            ..Default::default()
        }
        .apply(&original);
        store.save(changed).unwrap();
        store.preferences = None;
        let loaded = store.preferences().unwrap();
        assert_eq!(loaded.size, 600);
        assert_eq!(loaded.position, original.position);
        assert!(!loaded.enabled);
    }

    #[test]
    fn invalid_configuration_is_reported_and_never_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("settings.json");
        std::fs::write(&file, b"broken json").unwrap();
        let mut store = PetStore::new(temp.path().to_path_buf(), PathBuf::new());
        assert!(store.preferences().is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"broken json");
        assert!(Preferences {
            size: 50,
            ..Default::default()
        }
        .validate(&store.catalog)
        .is_err());
    }

    #[test]
    fn paths_cannot_escape_the_managed_directory() {
        let temp = tempfile::tempdir().unwrap();
        for invalid in [
            "../file",
            "/file",
            "C:/file",
            "..\\file",
            "",
            "a/../../file",
        ] {
            assert!(managed_path(temp.path(), invalid).is_err(), "{invalid}");
        }
        assert_eq!(
            managed_path(temp.path(), "webm/写代码.webm").unwrap(),
            temp.path().join("webm/写代码.webm")
        );
    }

    #[test]
    fn monitor_removal_and_negative_coordinates_keep_the_pet_visible() {
        let areas = [Rect {
            x: -1920.0,
            y: 0.0,
            width: 1920.0,
            height: 1040.0,
        }];
        assert_eq!(
            clamp_position(Position { x: -800, y: 100 }, 462.0, 388.0, &areas),
            Position { x: -800, y: 100 }
        );
        assert_eq!(
            clamp_position(Position { x: 2500, y: 2000 }, 462.0, 388.0, &areas),
            Position { x: -462, y: 652 }
        );
    }
}
