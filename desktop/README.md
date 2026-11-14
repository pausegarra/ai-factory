# Desktop frontend

React and TypeScript UI lives under `src/context/factory`. Workflow business rules stay in `domain`; Tauri IPC and React components stay in `infrastructure`.

## Workflow activation

Open **Workflows** and choose a trigger. Activation controls are available without selecting a step:

- **Enable workflow globally** controls every account for that trigger.
- Each GitHub account checkbox stores its own preference, keyed by the stable profile ID, not the account label or login. Missing preferences default to enabled.
- Global disable takes precedence. Account checkboxes remain editable while globally disabled; their textual status shows the effective state.
- Changes remain local until **Save workflow**. **Discard** restores the current persisted configuration. A failed save keeps the draft for correction and retry.
- Saving locks the editor and navigation. **Sync now** stays available and uses persisted settings, not the unsaved draft.
- New runs and retries use saved activation. Queued and running tasks continue; use **Cancel** to stop an existing task.
- Reactivation discovers work that remains eligible. GitHub events that are no longer observable are not replayed.
- Missing tokens are reported separately: activation does not confirm a working connection.
- Unavailable profile IDs remain visible and can be removed. Their preference has no effect until that exact profile ID exists again.

Settings contains account credentials, not workflow activation controls. Saving unrelated Settings fields preserves the latest saved workflow maps and validates those workflows.

The optional wire field is `profile_activation?: Record<string, boolean>`. Old settings without it retain global behavior. `constructor` and `__proto__` are valid profile IDs; only own map properties count. No new Tauri commands are required.

## Verification

```sh
npm --prefix desktop ci
npm --prefix desktop test
npm --prefix desktop run build
```

Tests use fixtures and mocked IPC, never live GitHub accounts. Activation coverage includes domain precedence and validation, keyboard/native controls, orphan removal, save/discard/failure/busy behavior, canvas callbacks, settings events, cross-page persistence, retry rejection, and settings IPC roundtrips.

For visual review, verify focus outlines on global/account checkboxes, account groups at narrow widths, and the busy editor in a browser or desktop preview. Automated DOM tests do not establish screenshot or native Tauri runtime verification.

### Agent usage

Choose **View Usage** at the bottom of the sidebar to query every configured Codex account (including accounts without a GitHub profile binding) and the configured OpenCode CLI. **Refresh** requests a new snapshot while the dialog is open. Nothing is polled in the background and no usage history is saved.

Codex remaining percentages are calculated as 100% minus reported usage for each quota window. Absolute token amounts are not inferred. Pending and failed accounts stay visible without invented percentages. Failed refreshes retain previous valid values with their original check time and a stale notice.

OpenCode uses the experimental Console/Go usage API through a temporary private CLI process. Reported percentages keep their original meaning; the dialog explains when a remaining percentage cannot be established. Historical `stats` are never interpreted as remaining quota. Versions without compatible API/standalone support show an explanation. An explicit absent Console session does not imply that credentials for other providers are missing.

The dialog supports Escape, contained Tab/Shift+Tab navigation, initial close-button focus and focus restoration. Opening it preserves the underlying page, drafts and selected run. Browser preview reports that the desktop app is required without invoking native account queries. Tests use synthetic protocols, managed temporary homes and fake CLI processes. Headless DOM tests cover semantics and interaction; native rendering, viewport geometry and actual account compatibility are not asserted by those tests.
