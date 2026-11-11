# AI Factory

Desktop app for local GitHub automation through visual workflows and CLI agents. Built with Tauri 2, Rust, React, React Flow and Tailwind CSS. English interface, dark theme. GitHub uses REST API tokens per account; the desktop app needs neither `gh` nor Python.

## Run locally

Install Node.js 22.12+, Rust 1.93.1+, Git and the [native Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). Install and authenticate Codex CLI or OpenCode CLI for the agents you use.

```sh
npm ci
npm --prefix desktop ci
npm run dev
```

`npm run dev` starts the native app and Rust engine. `npm --prefix desktop run dev` starts an explicit browser preview without a backend. Commands can be configured as absolute executable paths when your desktop session has a limited PATH.

## Set up your factory

1. Add GitHub token profiles in Settings. Each token must belong to its configured login. Multiple accounts are supported; the issue author or assigned reviewer selects the account for a run.
2. Set allowed owners, issue authors and reviewers. An empty reviewer list skips review request discovery.
3. Repositories are downloaded automatically from GitHub using the event actor's token. No local clone or folder mapping is required.
4. Edit the three Flows. Add/delete/drag nodes, connect dependencies, select Codex or OpenCode, model, effort, access, prompt and result JSON Schema. Save to apply changes to new runs.
5. Set global concurrency, GitHub polling interval and node timeout. Enable startup at login if required.

Tokens are never returned to the UI after storage. An empty replacement field preserves the existing token; removing its profile removes the stored credential. Tokens need access to the repositories and the corresponding contents, issues and pull request operations. Factory verifies token identity and uses process-local Git credentials without changing global account settings.

GitHub tokens are stored in macOS Keychain, Windows Credential Manager or Linux Secret Service. SQLite stores account metadata and opaque credential references. There is no plaintext fallback: unlock the system credential store if saving or reading a token fails. Linux needs a running user D-Bus session and an unlocked Secret Service provider (such as GNOME Keyring or KWallet). Each GitHub operation retrieves only its actor's credential; an unavailable profile does not disable the scheduler or unrelated accounts. Native credential reads run outside the async executor. Historical logs are withheld if required secrets cannot be loaded for redaction.

On first launch after upgrading, existing plaintext tokens are written to the native store and read back before settings are replaced. A failed migration keeps the previous settings available for retry. Successful migration scrubs the live SQLite database and truncates its WAL; it cannot erase external backups or filesystem snapshots. Token rotation uses a new verified entry before switching references. Failed cleanup is queued and retried on the next save or launch. Credentials are local to the OS account and do not travel with a copied SQLite file; re-enter tokens on another machine. The engine's `Credentials` port can be supplied by a server secret manager; tests use an injected in-memory store.

## Three triggers

- **New issue:** open issues with titles starting with `[AI]`, in configured owners, from allowed authors.
- **PR approved:** only PRs published and verified by an issue workflow. A review must approve the current head SHA. Alternatively, a new conversation comment exactly `OK` after trimming whitespace and ignoring case is accepted when its author is both the PR author and source issue author. A head creates at most one event; consumed comment IDs remain consumed across restarts and head changes.
- **Review requested:** open PRs with a request directed to a configured reviewer. After observing withdrawal, a fresh request creates another event. The PR need not originate from an issue workflow.

Each trigger has its own editable DAG and enabled switch. Nodes wait for successful dependencies; blocked or failed nodes prevent descendants from executing. Writers are serialized within each run. Per-flow node parallelism is configurable. The default global limit is **4 runs**, shared by all triggers; additional tasks wait in SQLite.

Model menus are loaded from the selected installed CLI. Codex supplies its available models and reasoning efforts; OpenCode 2 supplies enabled provider models and their variants. Refresh reloads the catalog. Changing agent clears model and effort; changing model clears effort. Discovery failures keep saved values visible and allow retry. An empty model keeps the CLI default.

Dependencies are edited only on the canvas: drag a step’s output dot to another step’s input dot. Each connection is one dependency; select an arrow and press Delete to remove it. Parallel dependents display as separate branches. Arrange flow restores a dependency-based layout; manually moved node positions are preserved.

Runs keep immutable flow snapshots. Retrying creates a new attempt using the current flow. Uncertain sessions after an unexpected exit are interrupted and require an explicit retry.

Each execution gets a fresh clone in `workspaces/<run-id>` under the application data directory. Nodes in that execution share its clone; concurrent issues and retry attempts use different directories. PR workflows fetch and verify the expected head SHA. GitHub tokens stay in process-local Git configuration and are not embedded in remote URLs.

After all agent and Git processes stop, Factory removes the clone on success, failure, timeout or cancellation. Startup also removes leftovers belonging to known completed/interrupted runs; unrelated directories and queued runs are preserved. Cleanup failures remain visible and are retried at the next launch. Existing legacy local-folder settings are accepted but no longer used.

History, session logs, structured results and project memory stay outside temporary clones. Declared artifacts are validated, copied into persistent per-run memory, and returned with their archived paths and hashes so downstream steps and history retain them. Unpublished changes and undeclared files in a workspace are removed when the execution ends; workflows that implement code should include their publishing step before completion.

## Live operations

Overview and Runs show queued, active and historical tasks. Run details include step states, session identity, results and live logs, with cancel/retry actions. Rust emits Tauri events for creation, lifecycle changes and output. The UI subscribes before reading its initial snapshot and repeats reads if events arrive during a request. Event lag forces resynchronization.

**Delete run** permanently removes one stopped attempt and its local logs, results,
artifacts, private memory and remaining managed workspace. Cancel queued or running
runs first, then wait for their processes and cleanup to finish. Confirm deletion
from run details; there is no undo.

GitHub resources, other attempts, shared import archives and minimal automation
safety records are kept. Deleting history does not replay consumed events or remove
verified PR provenance. Pending deletions show their run ID and a **Retry deletion**
action; restarting Factory also resumes cleanup, never the workflow. Local deletion
does not require a GitHub token or network access.

External source files, CLI-owned sessions, backups and copies already embedded in
other runs are outside this deletion. A later explicit legacy import can restore
imported history from its unchanged source archive.

Closing the window leaves Factory in the system tray. Choose **Quit AI Factory** from the tray to stop the engine and child processes. Startup at login uses each OS's native startup mechanism and opens in the background.

## Agent results and GitHub actions

Codex uses `codex exec --json`, access-specific sandboxing and an output schema. OpenCode uses `opencode run --format json`, model/variant and per-node tool permissions. The prompt includes event context, workspace, project memory and dependency results.

```json
{
  "status": "succeeded",
  "summary": "Work completed and checks performed",
  "artifacts": [],
  "data": {}
}
```

Valid statuses are `succeeded`, `blocked` and `failed`. `data` must satisfy the node schema. `data.approved=false` cannot be declared successful. Artifacts include `path`, `sha256` and `kind`; Factory validates authorized paths, existence and hashes.

Agents request GitHub operations through `data`; the engine performs REST calls with the actor token:

- `publish: {title, body, base}` pushes the issue branch and creates or reuses its PR. The body must close the source issue using `Closes #N`.
- `merge: {method: "squash"}` in an approved PR flow, guarded by its head SHA. After confirming the merge, Factory deletes only the verified remote source branch in the same repository, never the target or default branch.
- `review: {event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT", body}` in a review flow.
- `comment: "text"` comments on the event.

Factory validates every requested action before performing any mutation. Blocked results can report comments or request changes, but cannot publish, merge or approve. Retries reconcile existing PRs/merges and use durable receipts plus hidden markers for comments and reviews, including recovery after a lost API response. Cancellation is checked before new writes.

Source-branch cleanup is part of completing the existing merge action; saved workflows need no new action or opt-in. Factory rechecks PR identity, target, default branch and the source reference's approved SHA before deletion. Fork cleanup and local-branch deletion are not supported, and branch protections are never bypassed. An already absent source is accepted only after verifying repository and reference access. If merge succeeds but cleanup fails, the action reports a partial failure without undoing the merge or running later actions as if cleanup succeeded. Use the existing explicit retry to finish cleanup without repeating the merge. A source branch that has visibly moved is preserved. GitHub deletion has no conditional SHA guard, so a concurrent push between the final read and deletion remains a limitation.

Default flow prompts explain these contracts. No `gh` process is used by the desktop app.

Every node also receives a shared unattended execution instruction, including nodes in saved or custom workflows. Agents complete their assigned role without asking for human spec/plan approval, clarification or execution-mode choices: spec authors deliver the specification, planners deliver the plan, builders finish implementation/build/verification, and reviewers return a decision. Skills' interactive checkpoints are delegated to downstream workflow reviews. Nonblocking ambiguity is resolved with documented assumptions; genuine blockers return a structured `blocked` result instead of waiting for a reply. Access restrictions and actual upstream review decisions still apply. Codex uses `approval_policy="never"`; OpenCode runs automatically with its question tool denied.

## Local data and migration

All app-owned data lives in the OS application data directory:

- macOS: `~/Library/Application Support/dev.aifactory.desktop`
- Windows: `%APPDATA%` followed by `dev.aifactory.desktop`
- Ubuntu: `~/.local/share/dev.aifactory.desktop`, respecting `XDG_DATA_HOME`.

This includes `factory.sqlite3`, settings, temporary workspaces, persistent project memory, prompts, results and session logs. Settings shows the exact path. Factory clones directly from GitHub and does not modify local repositories.

Settings provides an explicit legacy YAML and/or SQLite import. The source database opens read-only. Available history, flow snapshots, logs, memory, PR provenance and trigger/comment/review deduplication are preserved. Legacy pending or active work is imported as interrupted for review before retrying. Imports are idempotent and report missing data with warnings and an archive of source tables. Tokens previously managed through `gh` must be added in Settings.

The previous daemon and its Python dependencies have been removed. The importer is implemented entirely in Rust; importing old configuration or history does not require Python.

## Check and build

```sh
npm --prefix desktop test
npm --prefix desktop run build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
npm run build
```

Build installers locally on native hosts: Windows x64 NSIS, macOS Apple Silicon app/DMG, and Ubuntu x64 deb/AppImage. Local bundles live under `target/release/bundle`, or `target/<target>/release/bundle` for an explicit target. Public signed distribution requires your own signing credentials.

The [Ubuntu Dockerfile](.ci/ubuntu-build.Dockerfile) prepares a reproducible Ubuntu 22.04 x64 build host for Docker-based verification from macOS. Engine tests use local HTTP fixtures and fake agent CLI processes; they do not publish to GitHub.


For Docker verification on Apple Silicon with Docker's Rosetta support enabled:

```sh
docker build --platform linux/amd64 -f .ci/ubuntu-build.Dockerfile -t ai-factory-ubuntu-build:22.04 .
docker run --rm --init --platform linux/amd64 \
  -e AI_FACTORY_DOCKER_ROSETTA=1 \
  -v "$PWD:/source:ro" -v ai-factory-ubuntu-work:/work \
  ai-factory-ubuntu-build:22.04 bash /source/.ci/verify-ubuntu.sh
```

The opt-in flag lets the verifier normalize three ELF padding bytes in the downloaded AppImage build helper if Rosetta rejects its AppImage marker. It retries packaging only after a confirmed linuxdeploy failure. Application code and generated installers are not patched by this workaround. Native x64 Linux hosts do not need the flag.
