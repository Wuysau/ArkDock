# ArkDock

鸿蒙外置贴边预览伴侣。

ArkDock is a Windows desktop companion for previewing a real HarmonyOS application beside the window where it is being developed. It stays an independent top-level window and does not inject into, embed in, or modify the target application.

The current `dev` source includes magnetic docking effects, Orb/Hidden display modes, a native ArkUI Inspector, selection-context support, and binary Previewer frame rendering. These features have not all completed integrated Windows acceptance together; see [Compatibility](docs/compatibility.md) for their current validation status.

## What it does

- Follows an ordinary application or terminal in a separate ArkDock window.
- Discovers project candidates as read-only evidence; project association and permission to run its build logic are separate decisions.
- Uses a real HarmonyOS Previewer runtime for supported configurations. A browser mock is never presented as an ArkUI preview.
- Provides component inspection and selection-context tools when the selected runtime exposes the required information.
- Provides explicitly paired Agent transports and conservative Windows assisted-input fallback. Current-conversation sending depends on the host's verified interface and authorization; see [Agent delivery and its validation limits](docs/agent-delivery/README.md). Copying or filling text is not a send receipt.

The supported native profile uses HarmonyOS SDK/Previewer 26.0.0.105 (API 26), verified tool fingerprints, a default Stage target, `pages/Index`, and a 360×640 surface. It can build and run Previewer without opening DevEco Studio GUI. Preview rebuilds restart the runtime and reset component state; they are not state-preserving hot reload.

The source also contains an opt-in experimental adapter for the public OpenHarmony Previewer control protocol: continuous touch, committed Unicode/key input, app color mode, admitted compiled-page navigation, and native orientation/resolution changes. These additions are **NOT_RUN — awaiting user acceptance** on the commercial SDK; command acknowledgements, frame observation, and configuration confirmation remain separate. Full native IME, additional device-profile restarts, and emulator/physical-device execution are blocked by their documented interface or runtime requirements. See [Native controls and per-feature acceptance steps](docs/native-controls/README.md).

ArkDock currently targets Windows 11 x64. WSL and remote project identities are retained, but native preview execution for those environments is unsupported. Simulator, device, arbitrary SDK versions, state-preserving hot reload, and full IME input are not claimed as supported.

## Build from source

See [Windows development](docs/building-windows.md) for prerequisites and commands. The short path is:

```powershell
pnpm install --frozen-lockfile
pnpm run doctor
pnpm verify
pnpm desktop:compile
```

Real native preview additionally requires a HarmonyOS Command Line Tools installation that you obtained and are authorized to use. ArkDock does not bundle or redistribute the SDK. Configure its full root in ArkDock's toolchain settings; see [Compatibility](docs/compatibility.md) for the supported scope and limitations.

`pnpm desktop:build` also creates installer bundles. An installed app needs WebView2, but it does not need the source-development Node, pnpm, Rust, or Vite tools. Native preview still needs the user's licensed HarmonyOS toolchain.

`pnpm verify:native` checks a real desktop-to-SDK flow; `pnpm verify:release` additionally applies the release and packaging gates. Their `PASS`, `FAIL`, and `BLOCKED` results have distinct meanings. `pnpm run doctor` is the project diagnostic script; pnpm's built-in `doctor` command is separate.

## Documentation

- [Product and safety boundaries](docs/product.md)
- [Architecture](docs/architecture.md)
- [Compatibility and validation status](docs/compatibility.md)
- [Windows development and build](docs/building-windows.md)
- [Multi-Agent development and integration](docs/development/agent-workflow.md)
- [Native preview frontend contract](docs/native-preview/frontend-contract.md)
- [Windows acceptance guide](docs/testing/windows-acceptance.md)
- [Native counter sample](examples/native-counter)
- [Optional weak session-registration helper](docs/runbooks/session-registration.md)

## License

ArkDock's own source code is licensed under [Apache License 2.0](LICENSE). It permits lawful commercial use, modification, and redistribution subject to the license terms; no open-source license can prevent every unauthorized or unlawful use. Third-party components retain their separate licenses in `src-tauri/resources/third-party-notices.txt`. The HarmonyOS SDK and Previewer are not covered by ArkDock's license and are not distributed with the source.

## Contributing

Issues and pull requests are welcome. Please include the affected Windows version, concise reproduction steps, and sanitized logs when reporting a defect. Never attach SDK binaries, credentials, private project data, or unsanitized user paths.
