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
