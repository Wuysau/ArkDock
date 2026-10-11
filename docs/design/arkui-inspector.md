# Native ArkUI Inspector

The Inspector reads component metadata from a real Previewer runtime. Runtime support is profile-specific: unknown commands or schemas must remain unavailable rather than being guessed. A browser-rendered tree is not a substitute.

## Adapter and bounds

`src-tauri/src/inspector.rs` parses the supported component-tree response. The Windows Previewer adapter serializes inspection requests through its owned command channel; the frontend cannot send arbitrary native commands. Parsing is bounded by payload size, node count, depth, attribute count, and attribute length. Invalid or duplicate node IDs, malformed bounds, unknown schemas, and results from an old workspace/session/build fail closed.

The current wire interface does not provide an atomic identity shared by the component tree and image frame. `inspectionExactFrameMatch` therefore remains false. The current project, session, build, generation, and displayed-frame metadata must be checked, and the tree must be refreshed after image or layout changes. A matching runtime identity alone does not prove that a component rectangle corresponds to the exact displayed pixels.

## Interaction and hit testing

Inspection mode is separate from normal preview interaction. The overlay must not forward selection clicks as Previewer application clicks. Returning to interaction mode restores normal input. Selection uses cached native component bounds and hierarchy; visibility, clipping, and stacking fields are only used when the runtime supplies reliable values. When drawing order is unknown, the UI must not claim that a smallest rectangle is always the topmost component. Region selection remains the fallback.

Selection data refers to the displayed frame and current native session. Old selection state is invalid after a workspace, session, build, generation, or frame-content change. Source locations from runtime debug metadata are candidates until checked against the current trusted project and source revision.

## Agent boundary

An Inspector selection is data, not permission to write into an Agent. Manually entered conversation identifiers do not prove the active thread. The current source does not support sending to an active Codex or Claude conversation; copy, export, and read-only MCP access must not be reported as a submission.

IPC types remain generated from `protocol/model.json`. See [the frontend contract](../native-preview/frontend-contract.md) for transport and lifecycle details, and [Compatibility](../compatibility.md) for the current validation scope.
