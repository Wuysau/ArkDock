# Windows assisted input

ArkDock's assisted transport uses Windows UI Automation to fill a specifically
confirmed, empty composer. It does not submit the message. A verified fill is
`FILLED_PENDING_USER_SEND`, and the user checks and submits it in the target app.
There is no Agent receipt associated with this state.

## Current support

The user-selected Codex Desktop package `26.1007.2314.0` was inspected on Windows
on 2026-10-11. Its executable is `ChatGPT.exe` inside the **OpenAI.Codex** package;
the executable name alone cannot distinguish Codex from ChatGPT. The bounded
read-only UIA inspection found no editable composer. No text was filled in that
application, and no existing conversation was submitted to. This target therefore
uses **Copy modification request**. This result does not claim that every version,
screen, or accessibility configuration of Codex behaves identically.

Windows Terminal's editable surface cannot establish that the active application
is an Agent composer rather than a shell, password prompt, or another terminal
tab. ArkDock does not type or paste into terminals. Unknown applications also
fall back to explicit clipboard copying.

There is presently **no enabled production GUI composer contract**. The UIA
implementation and its native tests are complete, but enabling a real application
requires evidence for that version's composer identity, write semantics, and
control lifecycle. The test fixture is compiled into Rust tests only; it cannot
be selected as a supported Agent in production. A fixture PASS is not a Codex or
Claude compatibility claim.

## Safety and confirmation

The desktop supplies native HWNDs from the lifecycle-checked window controller;
the frontend cannot choose a raw handle. Inspection and fill check the HWND, PID,
process creation time, visibility, enabled state, top-level identity, executable
path and normal integrity level. Elevated, UIAccess, inaccessible, and unknown
targets are refused. No elevation is requested.

An offer contains a random, single-use confirmation ID, the target application
and PID, the identified composer label, and an expiry 30 seconds later. The
native challenge also binds the target and caller handles, full process identity,
UIA root and composer runtime IDs, and a monotonic expiry. Every fill attempt
consumes it, including a rejected attempt. The frontend presents the target before
the user explicitly confirms filling.

A foreground-event observer invalidates confirmation when the foreground HWND
changes, even if the user switches away and back. It observes handles only, not
keystrokes or other windows' text. Actor cancellation is checked throughout UIA
inspection and immediately before writing, allowing preview work to continue on
the desktop actor while the UIA worker is busy. Target/scope changes invalidate
pending work.

Discovery is bounded to the selected window, 256 nodes, 16 container levels and
three seconds, with 500 ms UIA connection/transaction limits. It traverses
container metadata without reading document/chat text, list items, passwords,
clipboard contents or private storage. Merely finding an Edit control grants no
permission: a verified app-specific composer contract must also match exactly.

Only that matched composer is checked for an empty value. Existing drafts are
preserved, including drafts entered after the confirmation offer. The sole write
is `IUIAutomationValuePattern::SetValue`, followed by comparison of the intended
value and runtime identity. The implementation never calls Enter, Invoke,
SendInput, WM_SETTEXT or SetFocus. If a write may have occurred but validation
fails, the result is `UNKNOWN`; the UI must ask the user to inspect the target
instead of automatically retrying. Windows UIA is not transactional: a provider
can change between individual calls, so it cannot guarantee atomic draft updates
or prove a conversation ID. Final user review remains necessary.

Microsoft documents that [ValuePattern.SetValue requires an enabled, writable
element](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationvaluepattern-setvalue).
That API contract alone does not establish that a control is a chat composer or
that an Agent consumed its contents.

## Verification

Run the ordinary validation and the explicit interactive Windows test separately:

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib agent::assisted
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib agent::assisted::windows::tests::live_owned_gui_fill_and_safety_contract -- --ignored --exact --nocapture
```

The second command creates an owned WPF GUI, focuses only its own windows, performs
real UIA writes, checks actual values, and closes its exact child process. It
verifies correct composer selection, no Enter or automatic submission, distinct
user confirmation, single-use tokens, expiration, wrong HWND with the same PID,
stale process identity, foreground switch-away-and-back, replaced UIA controls,
existing/late drafts, read-only/password controls, cancellation, destroyed targets,
and the fixture's rejection by production policy. Its evidence is
`artifacts/assisted-windows-fixture.json`.

The actual Codex inspection uses exact user-selected lifecycle parameters, never
the first process with a matching name:

```powershell
pwsh -NoProfile -File tests/native/assisted-probe.ps1 `
  -WindowHandle <selected-HWND> -TargetPid <selected-PID> -StartedAt <FILETIME>
```

This read-only script emits metadata only, performs no activation or input, and
requires the installed OpenAI.Codex package identity. The observed result is
recorded in `artifacts/assisted-codex-desktop-probe.json` as **BLOCKED**, separately
from the fixture's **PASS**. Run it only for the window the user has selected.
