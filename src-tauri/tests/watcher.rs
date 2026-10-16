use arkdock_core::scheduler::{watch_project, WatchEvent};
use std::fs;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

#[test]
fn operating_system_watcher_reports_ets_ignores_build_and_releases_its_channel() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("entry/src/main/ets")).unwrap();
    fs::write(temp.path().join("entry/build-profile.json5"), "{}").unwrap();
    fs::write(temp.path().join("entry/oh-package.json5"), "{}").unwrap();
    fs::create_dir(temp.path().join("build")).unwrap();
    let source = temp.path().join("entry/src/main/ets/Index.ets");
    fs::write(&source, "before").unwrap();
    let (sender, receiver) = mpsc::channel();
    let guard = watch_project(temp.path(), move |event| {
        let _ = sender.send(event);
    })
    .unwrap();
    fs::write(&source, "actual source change").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            WatchEvent::Changed(paths) if paths.iter().any(|path| path.ends_with("Index.ets")) => {
                break
            }
            WatchEvent::Error(error) => panic!("OS watcher failed: {error}"),
            _ => {}
        }
    }
    while receiver.recv_timeout(Duration::from_millis(200)).is_ok() {}
    fs::write(temp.path().join("build/generated.abc"), "generated").unwrap();
    // Hvigor rewrites this generated file at the module root during every build.
    // Treating it as a source edit would immediately schedule another build.
    fs::write(
        temp.path().join("entry/BuildProfile.ets"),
        "generated debug flags",
    )
    .unwrap();
    let generated_event = receiver.recv_timeout(Duration::from_millis(300));
    assert!(
        matches!(generated_event, Err(RecvTimeoutError::Timeout)),
        "unexpected generated-file event: {generated_event:?}"
    );
    let custom_source = temp.path().join("entry/src/main/ets/BuildProfile.ets");
    fs::write(&custom_source, "user authored source").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            WatchEvent::Changed(paths)
                if paths
                    .iter()
                    .any(|path| path.ends_with("entry/src/main/ets/BuildProfile.ets")) =>
            {
                break
            }
            WatchEvent::Error(error) => panic!("OS watcher failed: {error}"),
            _ => {}
        }
    }
    // Creation can produce several delayed OS notifications. Drain this edit
    // before testing watcher teardown, just as for the first source edit above.
    while receiver.recv_timeout(Duration::from_millis(200)).is_ok() {}
    drop(guard);
    while receiver.try_recv().is_ok() {}
    fs::write(&source, "after guard was dropped").unwrap();
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(1)),
        Err(RecvTimeoutError::Disconnected)
    ));
}
