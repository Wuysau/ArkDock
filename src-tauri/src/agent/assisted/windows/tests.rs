use super::*;
use crate::agent::assisted::AssistedController;
use crate::ipc::model::ProcessRef;
use base64::Engine;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

struct Fixture {
    child: Child,
    input: ChildStdin,
    output: mpsc::Receiver<serde_json::Value>,
    target: TargetInfo,
    handle: isize,
    other: isize,
}
impl Fixture {
    fn start() -> Self {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/native/assisted-fixture.ps1");
        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-STA", "-File"])
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start owned Windows UIA fixture");
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Ok(value) = serde_json::from_str(&line) {
                    if tx.send(value).is_err() {
                        break;
                    }
                }
            }
        });
        let ready: serde_json::Value = rx
            .recv_timeout(Duration::from_secs(20))
            .expect("fixture ready");
        let pid = ready["pid"].as_u64().unwrap() as u32;
        assert_eq!(child.id(), pid);
        let target = TargetInfo {
            id: "owned-fixture-window-1".into(),
            title: "ArkDock assisted owned fixture (not an Agent)".into(),
            app: "powershell.exe".into(),
            process: ProcessRef {
                pid,
                started_at: process_started_at(pid).unwrap().to_string(),
            },
        };
        Self {
            child,
            input,
            output: rx,
            target,
            handle: ready["first"].as_i64().unwrap() as isize,
            other: ready["second"].as_i64().unwrap() as isize,
        }
    }
    fn command(&mut self, command: &str) -> serde_json::Value {
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
        self.output
            .recv_timeout(Duration::from_secs(5))
            .expect("fixture response")
    }
    fn controller(&self) -> AssistedController {
        AssistedController {
            pending: None,
            cancellation: None,
            fixture: Some(FixtureProfile {
                handle: self.handle,
                target: self.target.clone(),
            }),
        }
    }
    fn focus(&mut self) {
        self.command("focus");
        if unsafe { GetForegroundWindow() } as isize != self.handle {
            // Test-only focus action. Before each mouse action verify that the
            // point belongs to this exact owned fixture. Never click an Agent.
            use windows_sys::Win32::Foundation::{POINT, RECT};
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN,
                MOUSEEVENTF_LEFTUP, MOUSEINPUT, VK_LBUTTON,
            };
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                GetCursorPos, GetWindowRect, SetCursorPos, WindowFromPoint,
            };
            unsafe {
                validate_window(self.handle, &self.target).unwrap();
                let mut bounds: RECT = zeroed();
                assert_ne!(GetWindowRect(self.handle as HWND, &mut bounds), 0);
                let point = POINT {
                    x: bounds.left + 80,
                    y: bounds.top + 15,
                };
                assert_eq!(
                    GetAncestor(WindowFromPoint(point), GA_ROOT) as isize,
                    self.handle,
                    "only the owned fixture may be clicked"
                );
                let mut previous: POINT = zeroed();
                assert_ne!(GetCursorPos(&mut previous), 0);
                assert_ne!(SetCursorPos(point.x, point.y), 0);
                let input = [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP].map(|flags| INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0,
                            dy: 0,
                            mouseData: 0,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                });
                assert_eq!(
                    SendInput(
                        input.len() as u32,
                        input.as_ptr(),
                        size_of::<INPUT>() as i32
                    ),
                    2
                );
                // Mouse input is queued. Keep its verified target point in
                // place until foreground acknowledgement, then restore the
                // pointer; restoring it immediately can redirect queued input.
                let deadline = Instant::now() + Duration::from_secs(2);
                while (GetForegroundWindow() as isize != self.handle
                    || GetAsyncKeyState(i32::from(VK_LBUTTON)) < 0)
                    && Instant::now() < deadline
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                SetCursorPos(previous.x, previous.y);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { GetForegroundWindow() } as isize != self.handle && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            unsafe { GetForegroundWindow() } as isize,
            self.handle,
            "interactive desktop must permit owned fixture focus"
        );
    }
    fn value(report: &serde_json::Value) -> String {
        String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(report["valueBase64"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "close");
        let _ = self.input.flush();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Only this test's exact child handle is eligible for termination.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "requires interactive Windows desktop; creates and focuses only owned GUI fixture"]
fn live_owned_gui_fill_and_safety_contract() {
    let mut fixture = Fixture::start();
    fixture.focus();
    let mut controller = fixture.controller();
    let prompt = "ArkDock fixture request\nSelection ID: fixture-selection-1\n请调整组件间距。";
    let mut cases = Vec::new();
    let offer = controller
        .inspect(fixture.handle, &fixture.target, 0, 1000)
        .unwrap();
    let result = controller
        .fill(
            fixture.handle,
            &fixture.target,
            0,
            &offer.confirmation_id,
            prompt,
            1001,
        )
        .unwrap();
    assert_eq!(result.state, DeliveryState::FilledPendingUserSend);
    assert_eq!(result.receipt_id, None);
    let report = fixture.command("report");
    assert_eq!(Fixture::value(&report), prompt);
    assert_eq!(report["submissions"], 0);
    assert_eq!(report["enterEvents"], 0);
    cases.push("correct_window_exact_composer_value_no_enter_no_submit");
    assert!(controller
        .fill(
            fixture.handle,
            &fixture.target,
            0,
            &offer.confirmation_id,
            prompt,
            1002
        )
        .is_err());
    cases.push("single_use_confirmation");
    assert!(controller
        .inspect(fixture.handle, &fixture.target, 0, 2000)
        .unwrap_err()
        .contains("草稿"));
    assert_eq!(Fixture::value(&fixture.command("report")), prompt);
    cases.push("existing_draft_preserved");
    assert_eq!(fixture.command("user_submit")["submissions"], 1);
    cases.push("separate_fixture_user_confirmation_submits");

    fixture.command("reset");
    fixture.focus();
    let offer = controller
        .inspect(fixture.handle, &fixture.target, 0, 3000)
        .unwrap();
    assert!(controller
        .fill(
            fixture.other,
            &fixture.target,
            0,
            &offer.confirmation_id,
            prompt,
            3001
        )
        .is_err());
    assert_eq!(Fixture::value(&fixture.command("report")), "");
    cases.push("same_pid_wrong_hwnd_refused");

    let offer = controller
        .inspect(fixture.handle, &fixture.target, 0, 4000)
        .unwrap();
    let mut stale = fixture.target.clone();
    stale.process.started_at = "1".into();
    assert!(controller
        .fill(
            fixture.handle,
            &stale,
            0,
            &offer.confirmation_id,
            prompt,
            4001
        )
        .is_err());
    assert!(controller.inspect(fixture.handle, &stale, 0, 4002).is_err());
    cases.push("stale_process_lifecycle_refused");

    let offer = controller
        .inspect(fixture.handle, &fixture.target, 0, 5000)
        .unwrap();
    fixture.command("switch");
    // Event delivery is asynchronous; wait on observed invalidation, not a
    // guessed input delay. Returning to the same HWND must remain invalid.
    let deadline = Instant::now() + Duration::from_secs(2);
    while !controller
        .pending
        .as_ref()
        .unwrap()
        .foreground
        .changed
        .load(Ordering::SeqCst)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(controller
        .pending
        .as_ref()
        .unwrap()
        .foreground
        .changed
        .load(Ordering::SeqCst));
    fixture.focus();
    assert!(controller
        .fill(
            fixture.handle,
            &fixture.target,
            0,
            &offer.confirmation_id,
            prompt,
            5001
        )
        .is_err());
    assert_eq!(Fixture::value(&fixture.command("report")), "");
    cases.push("switch_away_and_back_cancels");

    let offer = controller
        .inspect(fixture.handle, &fixture.target, 0, 6000)
        .unwrap();
    fixture.command("replace");
    assert!(controller
        .fill(
            fixture.handle,
            &fixture.target,
            0,
            &offer.confirmation_id,
            prompt,
            6001
        )
        .is_err());
    cases.push("replaced_composer_refused");

    let offer = controller
        .inspect(fixture.handle, &fixture.target, 0, 7000)
        .unwrap();
    fixture.command("draft");
    assert!(controller
        .fill(
            fixture.handle,
            &fixture.target,
            0,
            &offer.confirmation_id,
            prompt,
            7001
        )
        .is_err());
    assert_eq!(
        Fixture::value(&fixture.command("report")),
        "user draft must be preserved"
    );
    cases.push("draft_added_after_confirmation_preserved");

    fixture.command("reset");
    let offer = controller
        .inspect(fixture.handle, &fixture.target, 0, 8000)
        .unwrap();
    assert!(controller
        .fill(
            fixture.handle,
            &fixture.target,
            0,
            &offer.confirmation_id,
            prompt,
            offer.expires_at
        )
        .is_err());
    cases.push("expired_confirmation_refused");

    fixture.command("readonly");
    assert!(controller
        .inspect(fixture.handle, &fixture.target, 0, 9000)
        .is_err());
    fixture.command("editable");
    cases.push("readonly_refused");
    fixture.command("password");
    assert!(controller
        .inspect(fixture.handle, &fixture.target, 0, 9500)
        .is_err());
    fixture.command("replace");
    cases.push("password_control_never_read_or_filled");

    let cancellation = Arc::new(AtomicBool::new(false));
    controller.set_cancellation(cancellation.clone());
    let offer = controller
        .inspect(fixture.handle, &fixture.target, 0, 9600)
        .unwrap();
    cancellation.store(true, Ordering::SeqCst);
    assert!(controller
        .fill(
            fixture.handle,
            &fixture.target,
            0,
            &offer.confirmation_id,
            prompt,
            9601
        )
        .is_err());
    assert_eq!(Fixture::value(&fixture.command("report")), "");
    assert!(controller
        .inspect(fixture.handle, &fixture.target, 0, 9602)
        .is_err());
    controller.set_cancellation(Arc::new(AtomicBool::new(false)));
    cases.push("scope_cancellation_refuses_late_fill_and_inspection");
    let mut product = AssistedController::default();
    assert!(product
        .inspect(fixture.handle, &fixture.target, 0, 10000)
        .is_err());
    assert_eq!(Fixture::value(&fixture.command("report")), "");
    cases.push("fixture_never_allowed_by_product_policy_safe_fallback");
    let final_report = fixture.command("report");
    assert_eq!(final_report["submissions"], 1);
    assert_eq!(final_report["enterEvents"], 0);
    let offer = controller
        .inspect(fixture.handle, &fixture.target, 0, 11000)
        .unwrap();
    writeln!(fixture.input, "close").unwrap();
    fixture.input.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while fixture.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(fixture.child.try_wait().unwrap().is_some());
    assert!(controller
        .fill(
            fixture.handle,
            &fixture.target,
            0,
            &offer.confirmation_id,
            prompt,
            11001
        )
        .is_err());
    cases.push("destroyed_target_refused");
    let evidence = serde_json::json!({
        "status": "PASS", "platform": "Windows", "mechanism": "UIAutomation ValuePattern.SetValue",
        "scope": "owned WPF fixture only; does not prove actual Agent GUI support",
        "cases": cases, "automaticSubmissions": 0, "enterEvents": 0,
        "explicitFixtureUserSubmissions": 1,
    });
    let artifacts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../artifacts");
    std::fs::create_dir_all(&artifacts).unwrap();
    std::fs::write(
        artifacts.join("assisted-windows-fixture.json"),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
    println!("{}", evidence);
}
