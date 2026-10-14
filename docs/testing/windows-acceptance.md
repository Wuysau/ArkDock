# Windows acceptance guide

This guide describes checks for a Windows build; it is not a record that those checks have passed. Record results in your own issue or test report and remove private project details before sharing logs or screenshots.

## Window behavior

1. Start ArkDock and drag it beside an ordinary application or terminal.
2. Move and resize the target and ArkDock. Confirm both remain independent windows and the target remains usable.
3. Exercise the available docking, Orb, Hidden, restore, minimize, and tray actions.
4. Check that closing or replacing the target invalidates its association and that stale state does not return when another window reuses the same title.
5. If a layout action would resize the target, verify ArkDock requests consent before applying it.

## Project and trust behavior

1. Try a target with no project path, one weak candidate, and multiple candidates. Weak or ambiguous evidence must require explicit selection.
2. Confirm that project association does not grant build trust.
3. Before previewing, verify the selected path and project identity, then grant trust explicitly.
4. Change or revoke trust and confirm the current preview session and input become invalid.
5. Confirm WSL and remote paths retain their environment identity and are not silently converted into local Windows paths.

## Native preview

Use a HarmonyOS Command Line Tools installation you are authorized to use. Configure its full root in ArkDock, select a real HarmonyOS project, and confirm the project trust separately.

Verify that the visible page comes from the actual Previewer runtime. For a supported interactive sample, check a real control changes state, a source edit produces a new frame, a build error is shown honestly, and a repaired source can recover. Stop or detach and confirm ArkDock releases only the processes and resources it created.

If the SDK is missing or its profile is unknown, the preview must stay blocked with a reason. Browser-only rendering, a screenshot, or a test fixture is not native-preview acceptance.

## Inspector and Agent delivery

Check that inspection mode does not trigger the preview application's normal click handler, that component selection is associated with the displayed frame, and that old selection state cannot cross a project or session change.

Agent delivery requires an explicitly paired transport, verified current-session scope, and a matching submission receipt. Check workspace/session changes, pairing cancellation or replacement, stale grants, ambiguous timeouts, and receipt correlation; none may silently redirect or retry a send. Copying text, filling an input, local files, a new conversation, and read-only MCP output are separate outcomes and do not prove submission to the active conversation. An unavailable host interface must remain unavailable; see [Agent delivery and setup](../agent-delivery/README.md) for transport-specific limits.

## Reporting

State the Windows version, display configuration, application build, selected SDK profile when used, exact action, expected and observed result, and whether the check was `PASS`, `FAIL`, `BLOCKED`, or `SKIPPED`. Sanitize user names, local paths, project content, window titles, and tokens before sharing diagnostics.
