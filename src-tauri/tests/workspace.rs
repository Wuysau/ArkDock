use arkdock_core::ipc::model::{Environment, ProjectRef};
use arkdock_core::workspace::WorkspaceStore;
use std::fs;

#[cfg(windows)]
#[test]
fn preferences_replace_durably_in_the_actual_windows_appdata_filesystem() {
    let appdata = std::env::var_os("APPDATA").expect("Windows AppData");
    let data = tempfile::Builder::new()
        .prefix("ArkDock-settings-test-")
        .tempdir_in(appdata)
        .unwrap();
    let app = tempfile::tempdir().unwrap();
    fs::write(app.path().join("build-profile.json5"), "{modules:[]}").unwrap();
    let project = project(app.path());
    let mut store = WorkspaceStore::open(data.path()).unwrap();
    store.remember(&project).unwrap();
    store.set_trust(&project, true).unwrap();
    assert!(WorkspaceStore::open(data.path())
        .unwrap()
        .is_trusted(&project));
    assert!(
        data.path().join("settings.json.bak").exists() || data.path().join("preferences").is_dir()
    );
}

#[test]
fn incomplete_preference_snapshot_cannot_grant_execution_trust() {
    let data = tempfile::tempdir().unwrap();
    fs::create_dir(data.path().join("preferences")).unwrap();
    fs::write(
        data.path().join("preferences/unfinished.json"),
        "{\"trusts\":[",
    )
    .unwrap();
    assert!(WorkspaceStore::open(data.path())
        .unwrap()
        .history()
        .is_empty());
    let committed = data.path().join(format!(
        "preferences/0000000000000001-{}.ready",
        "0".repeat(64)
    ));
    fs::write(committed, "").unwrap();
    assert!(
        WorkspaceStore::open(data.path()).is_err(),
        "Incomplete committed state fails closed"
    );
}

fn project(root: &std::path::Path) -> ProjectRef {
    ProjectRef {
        id: "fixture-entry".into(),
        name: "Fixture".into(),
        environment: Environment::WindowsLocal,
        workspace_root: root.to_string_lossy().into_owned(),
        app_root: root.to_string_lossy().into_owned(),
        module: "entry".into(),
        variant: "default".into(),
        sdk_id: None,
    }
}

#[test]
fn trust_is_explicit_revocable_and_invalidated_by_build_config_changes() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    fs::create_dir(&app).unwrap();
    fs::write(app.join("build-profile.json5"), "{modules:[]}").unwrap();
    fs::write(app.join("hvigorfile.ts"), "export default {};").unwrap();
    let project = project(&app);
    let mut store = WorkspaceStore::open(&dir.path().join("data")).unwrap();
    assert!(!store.is_trusted(&project));
    store.set_trust(&project, true).unwrap();
    assert!(store.is_trusted(&project));
    fs::write(app.join("hvigorfile.ts"), "export default {changed:true};").unwrap();
    assert!(!store.is_trusted(&project));
    store.set_trust(&project, true).unwrap();
    store.set_trust(&project, false).unwrap();
    assert!(!WorkspaceStore::open(&dir.path().join("data"))
        .unwrap()
        .is_trusted(&project));
}

#[test]
fn invalid_or_future_settings_are_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("settings.json"), r#"{"version":99}"#).unwrap();
    assert!(WorkspaceStore::open(dir.path()).is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("settings.json")).unwrap(),
        r#"{"version":99}"#
    );
}

#[test]
fn persistent_history_contains_no_runtime_window_or_process_identity() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = WorkspaceStore::open(&dir.path().join("data")).unwrap();
    let app = dir.path().join("app");
    fs::create_dir(&app).unwrap();
    fs::write(app.join("build-profile.json5"), "{}").unwrap();
    store.remember(&project(&app)).unwrap();
    let raw = fs::read_to_string(dir.path().join("data/settings.json")).unwrap();
    assert!(!raw.contains("pid"));
    assert!(!raw.contains("startedAt"));
    assert!(!raw.contains("hwnd"));
}

#[test]
fn declared_module_build_configuration_changes_revoke_trust() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app");
    fs::create_dir_all(app.join("entry/src/main")).unwrap();
    fs::write(
        app.join("build-profile.json5"),
        "{modules:[{name:'entry',srcPath:'./entry'}]}",
    )
    .unwrap();
    let project = project(&app);
    let mut store = WorkspaceStore::open(&dir.path().join("data")).unwrap();
    for name in [
        "entry/hvigorfile.ts",
        "entry/oh-package.json5",
        "entry/build-profile.json5",
        "entry/src/main/module.json5",
        "oh-package-lock.json5",
    ] {
        store.set_trust(&project, true).unwrap();
        fs::write(app.join(name), "changed configuration").unwrap();
        assert!(!store.is_trusted(&project), "trust remained for {name}");
    }
}
