# Claude Code Channel

ArkDock provides a local stdio MCP Channel in `tools/agent/claude-channel.mjs`.
It forwards explicitly submitted selections to the Claude Code process that
launched it. The Channel remains experimental, matching Anthropic's research
preview. Installing or enabling it requires the user's explicit consent.

## Protocol and support limits

The server declares `capabilities.experimental["claude/channel"]` and sends
`notifications/claude/channel` with text content and string metadata. It uses MCP
revision `2025-06-18`; it does not negotiate an unimplemented newer revision.
Claude does not acknowledge Channel notifications automatically. A successful
stdio write establishes `TRANSPORT_ACCEPTED`. Only a subsequent tool call from
the connected host can establish `AGENT_ACKNOWLEDGED` or `COMPLETED`.
See the [official Channel reference](https://code.claude.com/docs/en/channels-reference).

The installed CLI inspected during development was Claude Code **2.1.284**.
Its public `--help` output did not list the Channel switches. This check proves
the installed version only. Live Channel compatibility remains **BLOCKED** until
an authorized interactive host enables it and the acceptance procedure below
passes. A Node/HTTP test fixture cannot establish Claude compatibility.

Custom channels currently use
`--dangerously-load-development-channels server:arkdock` and an interactive
confirmation. Noninteractive `-p` does not enable this development bypass;
organization policy still applies. These prerequisites are controlled by
[Claude Code](https://code.claude.com/docs/en/channels).

## Session ownership and lifecycle

Every process creates a fresh Channel instance ID. Registration binds the
one-use ArkDock pairing token, current project, official
`CLAUDE_CODE_SESSION_ID`, and initialized stdio peer. The registered version
comes from MCP `clientInfo`. The Bridge's lease, project binding, selection
runtime lease, and send authorization remain independent checks.

An MCP subprocess retains its original session environment when the user runs
`/clear`. Startup with `--continue` or `--resume` without an explicit ID can also
report an initial startup ID. ArkDock therefore requires a matching official
SessionStart hook and invalidates the pairing on SessionEnd, changed session ID,
or changed working directory. This limitation is documented in Claude's
[environment-variable reference](https://code.claude.com/docs/en/env-vars).

`claude-lifecycle.mjs` reads only the documented hook input fields needed for
identity and lifecycle. It never opens the supplied transcript path. It writes
small, private guard files beside the ArkDock pairing file. SessionEnd writes
an irreversible local revocation marker before contacting `/v1/revoke`. Each
notification, data read, and receipt checks the guard. A changed or revoked
session requires fresh pairing; a new SessionStart cannot reactivate an old
token. Missing hooks block registration. See the
[official hook reference](https://code.claude.com/docs/en/hooks).

EOF closes the connection and revokes the Bridge registration. A failed poll
or an uncertain transport write stops delivery; the adapter does not resend an
event automatically. Revocation cannot retract a notification already written
to Claude. Such a race must remain an uncertain delivery until a valid receipt
exists. Disabling required lifecycle hooks invalidates the supported setup.

## Explicit setup

1. In ArkDock, select the intended project and target, then explicitly create a
   Claude Channel pairing. Save its returned JSON as a private local file. The
   schema is `endpoint`, `token`, `workspaceId`, `workspaceRoot`, and `expiresAt`
   (Unix milliseconds). The token grants access to this pairing. Keep this file
   outside source control; never include it in diagnostic output.
2. Generate configuration for review, using an absolute pairing file path:

   ```text
   node tools/agent/claude-channel.mjs --write-config --pairing-file <ABSOLUTE_PAIRING_FILE>
   ```

   This creates `<ABSOLUTE_PAIRING_FILE>.claude-mcp.json` and
   `<ABSOLUTE_PAIRING_FILE>.claude-settings.json`. Existing files are preserved.
   The generated files use the exact Node/script paths and structured arguments.
   They contain no bearer token. They register SessionStart, SessionEnd, and
   CwdChanged hooks using the official command `args` form, with no shell
   interpolation. The generator neither edits global Claude configuration nor
   starts Claude.
3. Review both files and consent to enabling this local server and its hooks.
   From the exact paired project root, start the intended interactive Claude
   session with both files and the development Channel switch:

   ```text
   claude --session-id <NEW_VALIDATION_UUID> --mcp-config <PAIRING_FILE>.claude-mcp.json --settings <PAIRING_FILE>.claude-settings.json --dangerously-load-development-channels server:arkdock
   ```

   Quote file paths using the terminal's normal argument rules. This example
   creates a new validation session; it does not write into a preexisting one.
   To enable a preexisting session that lacks the Channel, the user must stop
   its current process and explicitly restart that same session using
   `--resume <EXPLICIT_SESSION_UUID>` in place of `--session-id`. Never launch a
   concurrent resumed copy or treat it as the original active host. ArkDock
   does not perform either launch automatically.
4. Accept Claude's own Channel/MCP prompts. Check its Channel registration
   notice and `/mcp` status. If policy, version, authentication, or hook startup
   prevents registration, leave native delivery blocked.
5. In ArkDock, review the resulting registration and bind it explicitly. A
   single candidate is not selected automatically. Select a current native
   ArkUI component, enter the change request, and explicitly send it.

The tool has three CLI modes: normal stdio serving with `--pairing-file`,
configuration generation with `--write-config`, and official hook input on stdin
with `--hook`. Hook mode is configured by the generated settings file; it is
not a discovery mechanism.

## Selection and receipt tools

| Tool | Required arguments | Effect |
| --- | --- | --- |
| `arkdock.get_selection` | `deliveryId`, `selectionId` | Read authorized selection metadata |
| `arkdock.get_selection_context` | `deliveryId`, `selectionId` | Read the context and user instruction |
| `arkdock.get_selection_screenshot` | `deliveryId`, `selectionId`; optional `crop` | Read grant-authorized JPEG bytes |
| `arkdock.acknowledge_delivery` | `deliveryId` | Report that Claude consumed this event |
| `arkdock.complete_delivery` | `deliveryId`, `success`, `summary` | Report a processed result after acknowledgement |

The Channel reuses `SelectionProvider`: grants remain scoped to the selection,
workspace, preview session, allowed tool, expiry, and live runtime lease. Images
retain length and SHA-256 validation. Each delivery also names the exact grant
ID, and the grant pins the SHA-256 of its context file. Changing the instruction
revokes the prior grant; a queued delivery cannot adopt a newer delivery's
authorization. File paths and arbitrary source reads are
not tools. Fetching an image does not acknowledge delivery. Completion is an
Agent-reported result, not independent proof that code changes passed tests.
Busy-state detection and cancellation are not advertised.

Pairing expiry limits new registration. An already registered connection stays
usable while its session guard and Bridge lease remain valid. Acknowledgement
and completion can arrive after an edit rebuilds or expires the selected frame;
subsequent image/context reads still require a current runtime and grant.

## Validation

Run the bounded protocol suite from the repository root:

```text
node --test tests/portable/claude-channel.test.mjs
```

The suite exercises real Node child stdio pipes and authenticated loopback HTTP
against a labelled fixture, MCP initialization, selection integrity and stale
runtime rejection, foreign delivery rejection, deduplication, explicit receipt
ordering, EOF, lifecycle revocation, and safe configuration generation.

Windows additionally has an unignored cross-language integration test:

```text
cargo test --locked --manifest-path src-tauri/Cargo.toml --features native-tools --lib actual_rust_bridge_node_channel_and_selection_provider_roundtrip
```

This starts the real Rust Bridge, persists SelectionStore artifacts, launches
the actual Node Channel script, reads an authorized image through stdio, and
checks receipts after a runtime rebuild. Node must be on PATH. Its JPEG and
Claude peer are fixtures; this test does not enable or impersonate a real
Claude Code session.

Live Windows acceptance requires an explicitly enabled Claude host and a real
native ArkDock selection. Record the target process/session, project, delivery
ID, Channel event, explicit acknowledgement tool call, final reported result,
and relevant code verification separately. Repeat with `/clear`, `/resume`, a
project change, and a closed host: old bindings must stop accepting sends.
Preserve PASS / FAIL / BLOCKED / SKIPPED distinctly. Do not capture private
transcripts, credentials, or full screenshot content in public evidence.
