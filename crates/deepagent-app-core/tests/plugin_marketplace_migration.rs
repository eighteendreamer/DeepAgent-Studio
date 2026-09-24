//! 已退役的 GitHub topic 市场源的存量迁移回归。
//!
//! DeepSeek Harness 目录桥接（`https://github.com/topics/dsh-plugin`）已从本构建移除。
//! 老版本写入 `state.json` 的市场条目若不清理，会被当作普通 git 源重新物化并失败，
//! 因此必须在加载时被丢弃，同时不能误伤用户自行添加的市场。

use deepagent_app_core::plugin_loader::PluginRoots;
use deepagent_app_core::PluginService;

fn roots(root: &std::path::Path) -> PluginRoots {
    PluginRoots {
        session: Vec::new(),
        builtin: root.join("builtin"),
        workspace: None,
        personal: root.join("personal"),
        marketplace_cache: root.join("cache"),
        marketplaces: root.join("marketplaces"),
    }
}

fn write_state(state_path: &std::path::Path, marketplaces: serde_json::Value) {
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    std::fs::write(
        state_path,
        serde_json::json!({
            "version": 1,
            "enabled": {},
            "installed": {},
            "marketplaces": marketplaces,
            "healthChecks": {}
        })
        .to_string(),
    )
    .unwrap();
}

/// 两种 topic 写法都要丢弃，正常本地源必须保留。
#[test]
fn retired_topic_marketplaces_are_dropped_on_load() {
    let tmp = tempfile::tempdir().unwrap();
    let install_dir = tmp.path().join("app-data");
    let svc = PluginService::new(roots(tmp.path()), install_dir.clone());
    let local_source = tmp.path().join("team-marketplace").display().to_string();

    write_state(
        &install_dir.join("plugins").join("state.json"),
        serde_json::json!({
            "deepseek-harness": { "source": "https://github.com/topics/dsh-plugin" },
            "legacy-topic": { "source": "github-topic:dsh-plugin" },
            "team": { "source": local_source }
        }),
    );

    let names: Vec<String> = svc
        .list_marketplaces()
        .unwrap()
        .into_iter()
        .map(|marketplace| marketplace.name)
        .collect();

    assert_eq!(names, vec!["team".to_string()]);
}

/// 丢弃必须可重复，且不能把"市场为空"误判成错误。
#[test]
fn migration_is_idempotent_and_tolerates_empty_result() {
    let tmp = tempfile::tempdir().unwrap();
    let install_dir = tmp.path().join("app-data");
    let svc = PluginService::new(roots(tmp.path()), install_dir.clone());

    write_state(
        &install_dir.join("plugins").join("state.json"),
        serde_json::json!({
            "deepseek-harness": { "source": "https://github.com/topics/dsh-plugin/" }
        }),
    );

    for _ in 0..3 {
        assert!(svc.list_marketplaces().unwrap().is_empty());
    }
}

/// 加载只改内存；任何一次状态写入都要把清理结果固化到磁盘。
#[test]
fn next_state_write_persists_the_migration() {
    let tmp = tempfile::tempdir().unwrap();
    let install_dir = tmp.path().join("app-data");
    let svc = PluginService::new(roots(tmp.path()), install_dir.clone());
    let state_path = install_dir.join("plugins").join("state.json");

    write_state(
        &state_path,
        serde_json::json!({
            "deepseek-harness": { "source": "https://github.com/topics/dsh-plugin" }
        }),
    );
    assert!(state_path.is_file());

    // 走一次公开写路径（启停任意插件 id 都会读写 state）。
    let _ = svc.set_enabled("does-not-exist@team", false);

    let persisted = std::fs::read_to_string(&state_path).unwrap();
    assert!(
        !persisted.contains("github.com/topics"),
        "retired topic source survived a state write: {persisted}"
    );
}
