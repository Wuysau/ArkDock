# Compatibility and validation scope

ArkDock's native preview is deliberately limited to toolchain profiles that the project has verified. A successful compile or portable test does not establish Windows interaction, Inspector interoperability, or HarmonyOS runtime compatibility.

| Area | Current statement |
| --- | --- |
| Desktop | Windows 11 x64 is the primary target. The merged docking, particle effects, and Orb/Hidden flows still need integrated desktop acceptance. |
| Native Previewer | The current profile is HarmonyOS SDK/Previewer 26.0.0.105 (API 26), Hvigor 6.26.8, Windows x64, Stage target with initial page read from compiled resources, isolated custom route adaptation, 360×640 baseline and project-admitted device profiles. Tool fingerprints must match. Missing or unknown profiles remain blocked. A real backend check does not replace integrated GUI acceptance. |
| Native controls | Streaming touch, committed Unicode/key input, ColorMode, compiled-page routing, native orientation/resolution and controlled profile restart are wired. Ready input is enabled by default and pauseable. Host IME commits are supported; native preedit transactions are not. Real GUI behavior is awaiting user acceptance. See [native controls](native-controls/README.md) and [routing/performance](native-preview/routing-and-performance.md). |
| Device runtime | Independent background HDC lifecycle, signed HAP build/deployment, start/stop, verified foreground, real PNG capture and bounded input; local Emulator CLI lifecycle. Needs authorized targets, SDK tools and valid signing. Device GUI acceptance is NOT_RUN; capture refresh is not a high-FPS video stream. See [device runtime](native-controls/device-runtime.md). |
| ArkUI Inspector | Inspector UI and backend code are included. Portable and Rust tests cover contracts and state isolation; the merged Windows-to-SDK Inspector path has not yet been revalidated as a whole. |
| Agent delivery | Explicitly paired Claude Channel, managed Codex App Server and optional Desktop plugin transports are included. Managed Codex 0.159.2 received a real native ArkUI selection and images through the production Bridge; upstream stream errors blocked model completion. Live Claude and existing Desktop conversation acceptance remain **BLOCKED** until host-session delivery is verified. The observed Desktop UIA exposes no verified composer, so assisted input falls back to copy. See [delivery setup and limits](agent-delivery/README.md). |
| Displays and input | Single-display interaction is the target. Multi-display DPI, negative coordinates, elevated targets, and full keyboard/IME input are not claimed as verified. |
| Other environments | WSL and remote project identities are preserved, but native preview execution there is unsupported. Simulator and physical-device runtime require local Windows tools; real-device acceptance is still pending. |
| Refresh | Source changes use the supported build/preview lifecycle. State-preserving hot reload is not claimed. |

## What the current merge proves

The integrated source has portable tests, Rust tests, and compile/lint gates. Those checks validate contracts and code paths; they do not replace mouse-driven Windows acceptance, actual Previewer rendering, Inspector hit testing, or end-to-end Agent delivery. Until those integrated checks pass on the relevant desktop and SDK, the corresponding feature remains pending acceptance.

Native preview must show real ArkUI output from the user's legally obtained SDK. Browser UI, static screenshots, unit fixtures, and mock data cannot satisfy a native acceptance claim. ArkDock does not bundle the SDK.

For source setup see [Windows development](building-windows.md). For repeatable user-side checks see [Windows acceptance](testing/windows-acceptance.md).
