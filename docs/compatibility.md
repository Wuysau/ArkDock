# Compatibility and validation scope

ArkDock's native preview is deliberately limited to toolchain profiles that the project has verified. A successful compile or portable test does not establish Windows interaction, Inspector interoperability, or HarmonyOS runtime compatibility.

| Area | Current statement |
| --- | --- |
| Desktop | Windows 11 x64 is the primary target. The merged docking, particle effects, and Orb/Hidden flows still need integrated desktop acceptance. |
| Native Previewer | The current profile is HarmonyOS SDK/Previewer 26.0.0.105 (API 26), Hvigor 6.26.8, Windows x64, default Stage target, `pages/Index`, 360×640, click input and preview rebuild. Tool fingerprints must match. Missing or unknown profiles remain blocked. A real backend check does not replace integrated GUI acceptance. |
| Experimental native controls | Public protocol adapter for streaming touch, committed Unicode/key input, ColorMode, compiled-page routing and native orientation/resolution. Default disabled; all new SDK behavior is NOT_RUN — awaiting user acceptance. Acknowledgement is not rendering or semantic completion. Full native IME and nonbaseline profile restarts remain blocked. See [native controls](native-controls/README.md). |
| Device runtime | Separate HDC state, constrained command plans and target/deployment gates are present. HDC execution/shared-server ownership, installable HAP/signature provenance and actual device frames are not connected. Physical-device/emulator execution remains blocked; Previewer pixels are not device evidence. See [device runtime](native-controls/device-runtime.md). |
| ArkUI Inspector | Inspector UI and backend code are included. Portable and Rust tests cover contracts and state isolation; the merged Windows-to-SDK Inspector path has not yet been revalidated as a whole. |
| Agent delivery | Explicitly paired Claude Channel, managed Codex App Server and optional Desktop plugin transports are included. Managed Codex 0.159.2 received a real native ArkUI selection and images through the production Bridge; upstream stream errors blocked model completion. Live Claude and existing Desktop conversation acceptance remain **BLOCKED** until host-session delivery is verified. The observed Desktop UIA exposes no verified composer, so assisted input falls back to copy. See [delivery setup and limits](agent-delivery/README.md). |
| Displays and input | Single-display interaction is the target. Multi-display DPI, negative coordinates, elevated targets, and full keyboard/IME input are not claimed as verified. |
| Other environments | WSL and remote project identities are preserved, but native preview execution there is unsupported. Simulator and physical-device preview are not claimed. |
| Refresh | Source changes use the supported build/preview lifecycle. State-preserving hot reload is not claimed. |

## What the current merge proves

The integrated source has portable tests, Rust tests, and compile/lint gates. Those checks validate contracts and code paths; they do not replace mouse-driven Windows acceptance, actual Previewer rendering, Inspector hit testing, or end-to-end Agent delivery. Until those integrated checks pass on the relevant desktop and SDK, the corresponding feature remains pending acceptance.

Native preview must show real ArkUI output from the user's legally obtained SDK. Browser UI, static screenshots, unit fixtures, and mock data cannot satisfy a native acceptance claim. ArkDock does not bundle the SDK.

For source setup see [Windows development](building-windows.md). For repeatable user-side checks see [Windows acceptance](testing/windows-acceptance.md).
