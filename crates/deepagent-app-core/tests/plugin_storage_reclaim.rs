//! 插件市场下线后的存量存储回收回归。
//!
//! 市场特性已整体移除，`PluginRoots` 不再有 `marketplaces` / `marketplace_cache`
//! 字段，`PluginState` 也不再有 `marketplaces` 键。但早于该改动的安装会在
//! `plugins/` 下留下 `marketplaces/` 与 `cache/` 两个目录，且 UI 上已经没有任何
//! 入口能删它们。`PluginService::new` 负责一次性回收。

use deepagent_app_core::plugin_loader::PluginRoots;
use deepagent_app_core::PluginService;

fn roots(root: &std::path::Path) -> PluginRoots {
    PluginRoots {
        session: Vec::new(),
        builtin: root.join("builtin"),
        workspace: None,
        personal: root.join("personal"),
    }
}

/// 构造期必须回收两个废弃目录，且不能碰同级的其它数据。
#[test]
fn decommissioned_marketplace_storage_is_reclaimed_at_construction() {
    let tmp = tempfile::tempdir().unwrap();
    let install_dir = tmp.path().join("app-data");
    let plugins_dir = install_dir.join("plugins");

    let marketplaces = plugins_dir.join("marketplaces").join("old-market");
    let cache = plugins_dir.join("cache").join("old-market");
    let data = plugins_dir.join("data").join("demo@personal");
    std::fs::create_dir_all(&marketplaces).unwrap();
    std::fs::write(marketplaces.join("marketplace.json"), b"[]").unwrap();
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("payload"), b"x").unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("token"), b"keep-me").unwrap();

    let _ = PluginService::new(roots(tmp.path()), install_dir.clone());

    assert!(
        !plugins_dir.join("marketplaces").exists(),
        "decommissioned marketplaces dir survived"
    );
    assert!(
        !plugins_dir.join("cache").exists(),
        "decommissioned marketplace cache dir survived"
    );
    assert!(
        data.exists(),
        "reclaim must not touch per-plugin data under plugins/data"
    );
    assert_eq!(
        std::fs::read_to_string(data.join("token")).unwrap(),
        "keep-me"
    );
}

/// 回收必须幂等：目录不存在时构造不得报错。
#[test]
fn reclaim_is_idempotent_when_nothing_is_left() {
    let tmp = tempfile::tempdir().unwrap();
    let install_dir = tmp.path().join("app-data");

    for _ in 0..2 {
        let svc = PluginService::new(roots(tmp.path()), install_dir.clone());
        assert!(svc.list().unwrap().is_empty());
    }
}

/// 旧 state.json 里残留的 `marketplaces` 键必须被容忍，不得让加载失败。
#[test]
fn legacy_marketplaces_key_does_not_break_state_load() {
    let tmp = tempfile::tempdir().unwrap();
    let install_dir = tmp.path().join("app-data");
    let svc = PluginService::new(roots(tmp.path()), install_dir.clone());
    let state_path = install_dir.join("plugins").join("state.json");
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    std::fs::write(
        &state_path,
        serde_json::json!({
            "version": 1,
            "enabled": { "demo@personal": true },
            "installed": {},
            "marketplaces": {
                "deepseek-harness": { "source": "https://github.com/topics/dsh-plugin" }
            },
            "healthChecks": {}
        })
        .to_string(),
    )
    .unwrap();

    let list = svc.list().unwrap();
    assert!(list.is_empty());

    // 任何一次状态写入都要把这个已废弃的键从磁盘上抹掉。走真实安装路径，
    // 它一定会写 state；set_enabled 对不存在的 id 会先报错返回、不落盘。
    let source = tmp.path().join("src").join("demo");
    std::fs::create_dir_all(source.join("skills").join("demo-skill")).unwrap();
    std::fs::create_dir_all(source.join(".codex-plugin")).unwrap();
    std::fs::write(
        source.join(".codex-plugin").join("plugin.json"),
        serde_json::json!({ "name": "demo", "version": "0.1.0" }).to_string(),
    )
    .unwrap();
    std::fs::write(
        source.join("skills").join("demo-skill").join("SKILL.md"),
        "---\nname: demo-skill\ndescription: Demo\n---\nBody.",
    )
    .unwrap();
    svc.install_from_dir(&source).unwrap();

    let persisted = std::fs::read_to_string(&state_path).unwrap();
    assert!(
        !persisted.contains("deepseek-harness"),
        "legacy marketplaces key survived a state write: {persisted}"
    );
}
