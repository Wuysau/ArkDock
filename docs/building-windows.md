# Windows development

## Requirements

- Windows 11 x64.
- Node.js 24.18.0 or newer and pnpm 11.22.0.
- Rust stable with the MSVC target, Visual Studio C++ build tools, and the Windows SDK required by Tauri.
- WebView2 runtime for desktop development and use.
- A HarmonyOS Command Line Tools installation is optional for building ArkDock. It is required only for real native preview and must be obtained and used under its own license terms.

## Install, verify, and build

From the repository root, run:

```powershell
pnpm install --frozen-lockfile
pnpm verify
pnpm desktop:compile
```

`pnpm verify` runs the public source-tree guard, IPC generation check, portable tests, frontend checks, and Rust checks. `pnpm desktop:compile` builds the Windows desktop executable without generating installer bundles. `pnpm desktop:build` additionally creates the configured NSIS/MSI installers and may need the corresponding packaging tools. If a required dependency is unavailable, treat the result as blocked rather than as a passing check.

To run the app during development:

```powershell
pnpm dev
```

`pnpm dev:ui` serves only ArkDock's web interface; it does not provide native window control or a simulated HarmonyOS preview.

## Native Previewer setup

Install the HarmonyOS Command Line Tools from the official vendor distribution and review its terms. In ArkDock, select the full toolchain root in the device/toolchain settings. ArkDock probes the chosen location and enables only profiles recognized by the application. Do not point it at an SDK subdirectory or an unrelated directory. The SDK and Previewer are not included in this repository or its installers.

Project discovery is read-only. Select the intended project and grant build trust separately before allowing build logic to run. Unknown SDK versions or Previewer protocols remain blocked; changing an SDK manifest label does not make an unverified toolchain supported.

See [Compatibility](compatibility.md) for the current runtime scope and [the native IPC contract](native-preview/frontend-contract.md) for frame lifecycle details.

## Running the native gate

The native gate needs an interactive Windows desktop, a matched SDK, and a dedicated ArkDock instance from this checkout. Close DevEco Studio GUI yourself and keep the test windows visible. Start a separate validation instance in one terminal:

```powershell
$env:ARKDOCK_SDK = Read-Host 'Full root of your licensed Command Line Tools'
pnpm dev --validation
```

This mode uses a separate test identifier and WebView data directory, UI port 1424, and debugger port 9224. In another terminal, set the same SDK root and the PID of that exact dedicated ArkDock instance. Verify its executable path belongs to this checkout; never choose the first process by name. The instance must start detached, with no existing project/preview session.

```powershell
$env:ARKDOCK_SDK = Read-Host 'The same licensed Command Line Tools root'
$env:ARKDOCK_TEST_PID = Read-Host 'PID of the dedicated ArkDock test instance'
$env:ARKDOCK_TEST_PORT = '9224'
$env:ARKDOCK_TEST_URL = 'http://127.0.0.1:1424/'
pnpm verify:native
```

The driver verifies debugger ownership before IPC or mouse input, builds its own real counter sample, restores temporary source/trust changes, and cleans up only its owned resources. A covered target, missing SDK, unknown protocol, or unavailable desktop is `BLOCKED` (exit 2); an executed assertion failure is `FAIL` (exit 1); complete native acceptance is `PASS` (exit 0). `pnpm verify:release` also runs the public base checks and packages only after its required gates pass. Native acceptance does not prove multi-display DPI, Inspector UI, or Agent delivery claims beyond the executed checks.
