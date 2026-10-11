# Agent delivery

ArkDock keeps its independent preview window. Selected native ArkUI evidence and the user's instruction can be delivered through an explicitly paired Agent connection, filled into a verified Windows composer, or copied for manual submission. These actions have different receipts.

## Choose a connection

| Target | Connection | Current limit |
| --- | --- | --- |
| Claude Code | [Official Channel and session hooks](claude-channel.md) | Research Preview. Requires explicit interactive enablement; live host acceptance is pending. |
| Codex managed session | [Official App Server](codex.md) | Owns the explicitly created thread. Does not attach to an independently running CLI or Desktop conversation. |
| Codex Desktop | [Optional session plugin](codex.md) | Requires an enabled plugin, a per-conversation host identity and matching host workspace roots. Missing proof blocks direct sending. |
| Windows composer | [Assisted filling](assisted.md) | Only a verified composer profile can be offered. The user submits. Unknown controls and terminals fall back to copy. |

MCP context reads alone never submit a message. Window titles, HWNDs, process IDs and manually entered conversation IDs do not prove conversation ownership. Codex Desktop support is assessed separately from App Server and other OpenAI desktop products.

## In ArkDock

1. Bind the intended window, select and trust the actual HarmonyOS project, and select an element in its native preview.
2. Enter the modification request. Under **发送到 Agent → 连接与授权**, save a pairing file for the chosen Agent. Keep this credential outside Git and share it only with that local adapter.
3. Follow the target's setup guide. Pairing does not install an adapter or edit Agent settings. Host installation and enablement are separate, explicit actions.
4. Select the registered connection, verify its project and session, then **确认绑定此会话**. Later selections can reuse that confirmed binding while its connection remains valid.
5. **发送到当前 Agent** authorizes that selection, instruction and its two images for that connection. A busy or stale connection cannot silently choose another thread.

Without direct support, **检查当前目标输入框** either offers a short-lived target/composer confirmation or explains why it cannot fill. **填入 Agent 输入框** does not press Enter. **复制修改要求** explicitly replaces the system clipboard and requires manual paste and submission.

**撤销连接与授权** invalidates the pairing and selection grants. Already submitted Agent work cannot be retracted by revoking the local transport. Pairing files are transient bearer credentials; do not attach them to bug reports.

## Receipts

| State | Meaning |
| --- | --- |
| READY | Prepared; no external submission yet. |
| SENDING | Waiting for the bound transport to verify and hand off this request. |
| TRANSPORT_ACCEPTED | The Channel, plugin UI message API or App Server accepted its hand-off. |
| SUBMITTED | An official App Server user-message event identifies this delivery in its exact thread/turn. |
| AGENT_ACKNOWLEDGED | The connected Agent explicitly invoked the acknowledgment tool. A user-item event alone is insufficient. |
| COMPLETED | The Agent explicitly reported completion, or the managed App Server reported the correlated turn completed. This is not independent verification of the code change. |
| FILLED_PENDING_USER_SEND | The exact text was verified in the confirmed input field. The user must submit. |
| UNKNOWN | A write may have occurred, but its result is uncertain. Automatic resend is prohibited. |
| FAILED / SESSION_UNVERIFIED / UNSUPPORTED / EXPIRED | Rejected, missing ownership/capability, or stale before dispatch. The UI explains the reason. |

Transport and Agent acknowledgment receipts are retained separately. Rebuilding a preview invalidates image/context access but does not erase an already obtained message receipt. Local delivery history is bounded; handed-off records remain for up to one hour. An exhausted queue fails explicitly instead of evicting an uncertain delivery and resending it.

## Implementation boundaries

- `protocol/model.json` remains the only UI/native model source. Pair creation/revocation, registration binding and assisted inspect/fill commands carry the existing `RequestScope`.
- `agent/router.rs` retains bindings, stable idempotency and delivery history. The idempotency key covers the selection, instruction digest, exact Agent session and delivery mode; reconnecting cannot silently repeat an uncertain delivery.
- `agent/bridge.rs` starts only after explicit pairing. Its ephemeral IPv4 loopback HTTP endpoint requires a random 256-bit bearer token, exact Host, JSON POST, bounded request sizes and a total read deadline. Browser Origin/Cookie requests, transfer encoding and duplicate headers are rejected. No project execution endpoint exists.
- A pairing accepts one transport instance. Heartbeats expire after ten seconds; changed session, workspace, target lifecycle, trust or reconnection requires a new pairing. The native actor's separate runtime lease prevents a stalled actor from dispatching stale selections.
- Adapters receive only the explicitly sent selection grant and immutable context digest. The existing SelectionProvider checks current grant identity, context digest, native runtime lease, artifact bounds and hashes. Editing the instruction revokes earlier grants. Component strings remain untrusted technical evidence.
- UI Automation runs outside the preview actor, with bounded discovery, cancellation, lifecycle/privilege checks and a one-use composer confirmation. It cannot read conversation history or use an arbitrary Edit control as proof of an Agent composer.
- WSL/remote execution and independently started terminal conversations are not attachable through these Windows-local pairing files. Their environment identity is never converted to a local one.

The local user and the explicitly enabled adapter are the trust boundary; the bearer connection is not a sandbox against a malicious process already acting as that user. It grants no extra system privileges.

## Validation

Run `pnpm verify` for the generated contract, portable tests, Rust tests, Clippy and desktop compilation. Windows/native preview and live Agent acceptance are separate gates. The focused Rust Bridge tests exercise real loopback HTTP and a spawned Node Channel, while protocol fixtures never establish live Claude or Desktop support.

The managed Codex test reached real native ArkUI selection/image submission through ArkDock's production Bridge on CLI **0.159.2**. Upstream response-stream errors blocked successful model-turn completion. Claude Code **2.1.284** and Desktop **26.1007.2314.0** still require successful host-session acceptance before current-conversation delivery can be claimed. The observed Desktop UIA tree had no reliable editable composer, so that version currently uses copy fallback unless the plugin's host proof succeeds.
