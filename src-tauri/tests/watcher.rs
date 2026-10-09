use arkdock_core::scheduler::{watch_project, WatchEvent};
use std::fs;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

#[test]
fn operating_system_watcher_reports_ets_ignores_build_and_releases_its_channel() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("entry/src/main/ets")).unwrap();
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
    assert!(matches!(
        receiver.recv_timeout(Duration::from_millis(300)),
        Err(RecvTimeoutError::Timeout)
    ));
    drop(guard);
    while receiver.try_recv().is_ok() {}
    fs::write(&source, "after guard was dropped").unwrap();
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(1)),
        Err(RecvTimeoutError::Disconnected)
    ));
}
