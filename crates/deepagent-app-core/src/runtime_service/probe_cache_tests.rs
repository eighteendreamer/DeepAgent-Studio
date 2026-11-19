use super::tests::{compile_fake_runtime, runtime_binary_name};
use super::*;
use std::sync::{Barrier, OnceLock};

fn fixtures() -> &'static tempfile::TempDir {
    static FIXTURES: OnceLock<tempfile::TempDir> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        for (name, version) in [
            ("node", "22.1.0"),
            ("node_next", "24.21.0"),
            ("python", "3.12.1"),
            ("java", "17.0.12"),
        ] {
            compile_fake_runtime(dir.path(), name, version);
        }
        dir
    })
}

fn place_runtime(name: &str, path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::copy(fixtures().path().join(runtime_binary_name(name)), path).unwrap();
}

fn probe_count(executable: &Path) -> usize {
    std::fs::read_to_string(executable.with_extension("calls"))
        .unwrap_or_default()
        .lines()
        .count()
}

fn service(root: &Path) -> RuntimeService {
    RuntimeService::with_registry(
        &[root.join("runtimes")],
        Arc::new(UnavailableDownloader),
        vec![],
    )
    .with_state_root(root)
}

fn local_node() -> RuntimeRequirement {
    RuntimeRequirement {
        kind: RuntimeKind::Node,
        version: None,
        preference: RuntimePreference::LocalOnly,
    }
}

#[test]
fn environment_probe_cache_reuses_versions_across_service_restart() {
    let state = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut executables = Vec::new();
    for (kind, name) in [
        (RuntimeKind::Node, "node"),
        (RuntimeKind::Python, "python"),
        (RuntimeKind::Java, "java"),
    ] {
        let executable = project_runtime_candidates(kind, project.path()).remove(0);
        place_runtime(name, &executable);
        executables.push(executable);
    }
    let svc = service(state.path());
    let environment = svc.build_process_environment(Some(project.path()));
    assert_eq!(
        environment,
        svc.build_process_environment(Some(project.path()))
    );
    drop(svc);
    assert_eq!(
        environment,
        service(state.path()).build_process_environment(Some(project.path()))
    );
    for executable in executables {
        assert_eq!(
            probe_count(&executable),
            1,
            "unchanged runtime must execute --version only once, including after restart"
        );
    }
}

#[test]
fn environment_probe_cache_coalesces_concurrent_first_probe() {
    let state = tempfile::tempdir().unwrap();
    let node = state.path().join(runtime_binary_name("node"));
    place_runtime("node", &node);
    let svc = Arc::new(service(state.path()));
    let barrier = Arc::new(Barrier::new(8));
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let svc = svc.clone();
            let barrier = barrier.clone();
            let node = &node;
            scope.spawn(move || {
                barrier.wait();
                let found = svc
                    .resolve_runtime(&local_node(), None, Some(node))
                    .unwrap();
                assert_eq!(found.version, "22.1.0");
            });
        }
    });
    assert_eq!(
        probe_count(&node),
        1,
        "concurrent callers must join the probe"
    );
}

#[test]
fn environment_probe_cache_checks_each_version_requirement_without_reprobing() {
    let state = tempfile::tempdir().unwrap();
    let node = state.path().join(runtime_binary_name("node"));
    place_runtime("node", &node);
    let svc = service(state.path());
    let found = svc
        .resolve_runtime(&local_node(), None, Some(&node))
        .unwrap();
    assert_eq!(found.version, "22.1.0");
    let newer = RuntimeRequirement {
        version: Some(">=999999".into()),
        ..local_node()
    };
    assert!(svc.resolve_runtime(&newer, None, Some(&node)).is_err());
    assert_eq!(
        svc.resolve_runtime(&local_node(), None, Some(&node))
            .unwrap()
            .version,
        "22.1.0"
    );
    assert_eq!(
        probe_count(&node),
        1,
        "cache the version, never compatibility"
    );
}

#[test]
fn environment_probe_cache_refreshes_changed_executables_and_project_priority() {
    let state = tempfile::tempdir().unwrap();
    let project_a = tempfile::tempdir().unwrap();
    let project_b = tempfile::tempdir().unwrap();
    let node_a = project_runtime_candidates(RuntimeKind::Node, project_a.path()).remove(0);
    let node_b = project_runtime_candidates(RuntimeKind::Node, project_b.path()).remove(0);
    place_runtime("node", &node_a);
    place_runtime("node_next", &node_b);
    let svc = service(state.path());
    for (root, node, version) in [
        (project_a.path(), &node_a, "22.1.0"),
        (project_b.path(), &node_b, "24.21.0"),
    ] {
        let found = svc
            .resolve_runtime(&local_node(), Some(root), None)
            .unwrap();
        assert_eq!(found.executable, *node);
        assert_eq!(found.version, version);
    }

    let old_modified = std::fs::metadata(&node_a).unwrap().modified().unwrap();
    place_runtime("node_next", &node_a);
    std::fs::File::options()
        .write(true)
        .open(&node_a)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(old_modified + std::time::Duration::from_secs(2)),
        )
        .unwrap();
    let updated = svc
        .resolve_runtime(&local_node(), Some(project_a.path()), None)
        .unwrap();
    assert_eq!(updated.version, "24.21.0");
    assert_eq!(probe_count(&node_a), 2);

    std::fs::remove_file(&node_a).unwrap();
    let without_project = svc.resolve_runtime(&local_node(), Some(project_a.path()), None);
    assert!(without_project
        .map(|found| found.executable != node_a)
        .unwrap_or(true));
    place_runtime("node", &node_a);
    assert_eq!(
        svc.resolve_runtime(&local_node(), Some(project_a.path()), None)
            .unwrap()
            .version,
        "22.1.0"
    );
    assert_eq!(probe_count(&node_a), 3);
    assert_eq!(
        svc.resolve_runtime(&local_node(), Some(project_b.path()), None)
            .unwrap()
            .version,
        "24.21.0"
    );
    assert_eq!(probe_count(&node_b), 1);
}

#[test]
fn environment_probe_cache_tracks_path_changes_without_changing_parent_environment() {
    const CHILD_STATE: &str = "DEEPAGENT_RUNTIME_PROBE_TEST_STATE";
    if let Some(state) = std::env::var_os(CHILD_STATE) {
        let found = service(Path::new(&state))
            .resolve_runtime(&local_node(), None, None)
            .unwrap();
        assert_eq!(
            found.version,
            std::env::var("DEEPAGENT_RUNTIME_PROBE_TEST_VERSION").unwrap()
        );
        return;
    }
    let state = tempfile::tempdir().unwrap();
    let paths = [state.path().join("first"), state.path().join("second")];
    let nodes = paths
        .each_ref()
        .map(|path| path.join(runtime_binary_name("node")));
    place_runtime("node", &nodes[0]);
    place_runtime("node_next", &nodes[1]);
    for (path, expected) in [
        (&paths[0], "22.1.0"),
        (&paths[1], "24.21.0"),
        (&paths[0], "22.1.0"),
    ] {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime_service::probe_cache_tests::environment_probe_cache_tracks_path_changes_without_changing_parent_environment",
                "--nocapture",
            ])
            .env(CHILD_STATE, state.path())
            .env("DEEPAGENT_RUNTIME_PROBE_TEST_VERSION", expected)
            .env("PATH", path)
            .env("PATHEXT", ".EXE")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
    }
    assert_eq!(probe_count(&nodes[0]), 1);
    assert_eq!(probe_count(&nodes[1]), 1);
}

#[test]
fn environment_probe_cache_recovers_corrupt_disk_record() {
    let state = tempfile::tempdir().unwrap();
    let node = state.path().join(runtime_binary_name("node"));
    place_runtime("node", &node);
    service(state.path())
        .resolve_runtime(&local_node(), None, Some(&node))
        .unwrap();
    let records = std::fs::read_dir(state.path().join("cache/runtime-probes"))
        .expect("first successful probe must be persisted")
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    std::fs::write(&records[0], b"{interrupted record").unwrap();
    let restarted = service(state.path());
    assert_eq!(
        restarted
            .resolve_runtime(&local_node(), None, Some(&node))
            .unwrap()
            .version,
        "22.1.0"
    );
    restarted
        .resolve_runtime(&local_node(), None, Some(&node))
        .unwrap();
    assert_eq!(probe_count(&node), 2);
    let repaired: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&records[0]).unwrap()).unwrap();
    assert_eq!(repaired["version"], "22.1.0");
}

#[tokio::test]
async fn environment_probe_cache_refreshes_managed_install_uninstall_and_reinstall() {
    let state = tempfile::tempdir().unwrap();
    let binary = runtime_binary_name("node");
    let artifact = RuntimeArtifact {
        url: "https://example.invalid/node".into(),
        mirror_urls: vec![],
        sha256: None,
        dest_subdir: "node-sdk".into(),
        file_name: "node-sdk.tar.gz".into(),
        archive: ArchiveKind::TarGz,
        probe: Some(binary.clone()),
        files: vec![],
        allow_unpinned_files: false,
    };
    let mut entry = RuntimeEntry {
        id: "node-24".into(),
        name: "Node".into(),
        version: "22.1.0".into(),
        capability: "node-runtime".into(),
        size_bytes: 0,
        artifacts: HashMap::from([(Platform::current(), artifact)]),
        probe: binary,
        system_probe_paths: vec![],
    };
    let requirement = RuntimeRequirement {
        kind: RuntimeKind::Node,
        version: None,
        preference: RuntimePreference::ManagedOnly,
    };
    for (fixture, expected_version) in [("node", "22.1.0"), ("node_next", "24.21.0")] {
        let executable =
            std::fs::read(fixtures().path().join(runtime_binary_name(fixture))).unwrap();
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(executable.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, &entry.probe, &executable[..])
            .unwrap();
        let bytes = archive.into_inner().unwrap().finish().unwrap();
        entry.version = expected_version.into();
        entry
            .artifacts
            .get_mut(&Platform::current())
            .unwrap()
            .sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
        let svc = RuntimeService::with_registry(
            &[state.path().join("runtimes")],
            Arc::new(super::tests::BytesDownloader { bytes }),
            vec![entry.clone()],
        )
        .with_state_root(state.path());
        assert!(svc.resolve_runtime(&requirement, None, None).is_err());
        svc.install("node-24", Arc::new(|_, _| {})).await.unwrap();
        let found = svc.resolve_runtime(&requirement, None, None).unwrap();
        assert_eq!(found.version, expected_version);
        assert!(svc.status("node-24").unwrap().installed);
        assert_eq!(
            found,
            svc.resolve_runtime(&requirement, None, None).unwrap()
        );
        assert_eq!(probe_count(&found.executable), 1);
        assert!(svc.uninstall("node-24").unwrap());
        assert!(svc.resolve_runtime(&requirement, None, None).is_err());
    }
}
