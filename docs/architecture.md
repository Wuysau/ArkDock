# Architecture

ArkDock is a local-first Tauri 2 desktop application. Rust owns native windows, project discovery, trust, build scheduling, Previewer processes, Inspector queries, and frame validation. React renders backend snapshots and sends typed requests through the single frontend native entry point, `src/ipc/client.ts`.

```text
Windows window events ──> target lifecycle ──> project evidence and user choice
                                             └─> explicit execution trust
                                                    └─> serialized build session
                                                         └─> real Previewer frames
                                                              └─> typed IPC ──> React
```

## Key boundaries

- `protocol/model.json` is the IPC source of truth. `tools/generate-ipc.mjs` creates the Rust and TypeScript models; generated files are checked in and verified for drift.
- `src/ipc/` is the only product frontend entry to native commands. Components do not invoke Tauri directly.
- `src-tauri/src/windowing/` manages independent top-level windows, docking, layout, effects, and lifecycle.
- `src-tauri/src/discovery/` reads project evidence and returns candidates without running project configuration or build scripts.
- `src-tauri/src/workspace.rs` owns project identity, explicit trust, and persisted preferences.
- `src-tauri/src/scheduler.rs` serializes builds and rejects stale sessions, tasks, and frames.
- `src-tauri/src/preview/` probes supported toolchains and owns native Previewer execution and frame transport.
- `src-tauri/src/selection.rs` and `src-tauri/src/agent/` manage scoped Inspector selections and explicitly available Agent transports.

## State and data flow

Window identity is represented by opaque runtime tokens; raw native handles are not exposed to React. Project identity retains platform/environment, workspace, application, module, and build-configuration context. A previous association or project name cannot establish the current target's active project.

Build, preview, and selection operations carry the current target and session scope. The backend validates the scope and rejects late results. Binary frames are validated against workspace, session, build, generation, sequence, dimensions, and payload before display. A build failure may retain a visibly stale image, but stale pixels cannot receive input as if they belonged to the latest build.

Inspector selection uses backend-observed frame and component metadata. Source locations remain candidates until independently verified against the current trusted project. Agent session labels and manually entered thread IDs do not grant conversation write access; only an implemented, authorized transport with a matching receipt may report a submission.

## Native preview

Preview output must come from a real HarmonyOS Previewer runtime. The frontend frame store supports bounded binary frame transport and rejects stale frame identities. Browser-only UI development reports that the native bridge is unavailable and does not manufacture ArkUI pixels.

Toolchain identity is separate from project trust. Missing or unverified SDK/protocol support blocks native execution with a reason. ArkDock does not redistribute the HarmonyOS SDK or third-party tool binaries. See [Compatibility](compatibility.md) for current product claims and [the frontend contract](native-preview/frontend-contract.md) for the binary frame and IPC interface.
