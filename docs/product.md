# Product overview

ArkDock is an independent Windows desktop companion for people building HarmonyOS applications. It places a narrow preview window beside an ordinary application or terminal and can display pixels from a real, supported HarmonyOS Previewer runtime.

## User flow

1. Drag ArkDock beside a target window. ArkDock remains a separate top-level window.
2. Review project candidates discovered from read-only evidence, or select a project directory explicitly.
3. Grant build trust separately from project association.
4. Configure a legally obtained HarmonyOS toolchain and start a supported native preview.
5. Inspect or select visible components when the runtime provides the required metadata.

Candidates are not execution permission. A weak or ambiguous candidate is never silently treated as the active project. A changed project, process, session, build, or frame invalidates stale actions and results.

## Product boundaries

- The target can be an ordinary desktop application or terminal; the basic window and project flow does not depend on a specific Agent brand, plugin, or MCP server.
- ArkDock does not inject into or modify the target application and does not use `SetParent` to embed its window.
- Project discovery is read-only. Build execution requires explicit trust and a supported toolchain.
- Preview means actual ArkUI output from a HarmonyOS runtime. A browser-rendered imitation is not accepted as native preview.
- Windows, WSL, and remote paths keep their environment identities. Unknown or unsupported environments fail explicitly.
- The current Agent adapters do not support sending to an active Codex or Claude conversation. A copied prompt, local file, or read-only MCP response is not a submitted message.

## Current scope

The source includes window docking and motion effects, Orb/Hidden display modes, Inspector selection, binary frame transport, and native Previewer integration. Their merged behavior still requires integrated Windows acceptance; see [Compatibility](compatibility.md). ArkDock does not claim support for every SDK, device, page, input method, simulator, remote environment, or state-preserving hot reload.
