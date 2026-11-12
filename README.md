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
2. Set GitHub owners, allowed authors and allowed reviewers inside each token profile. A profile searches only its own owners with its own token. Include its GitHub login in allowed authors for issues and approval handling, or allowed reviewers for review requests. Empty owners disable discovery for that profile; an empty reviewer list skips its review request discovery.
3. Repositories are downloaded automatically from GitHub using the event actor's token. No local clone or folder mapping is required.
4. Edit the three Flows. Add/delete/drag nodes, connect dependencies, select Codex or OpenCode, model, effort, access, prompt and result JSON Schema. Save to apply changes to new runs.
5. Set global concurrency and GitHub polling interval. Agents run without a duration limit and can be cancelled manually. Enable startup at login if required.

Tokens are never returned to the UI after storage. An empty replacement field preserves the existing token; removing its profile removes the stored credential. Tokens need access to the repositories and the corresponding contents, issues and pull request operations. Factory verifies token identity and uses process-local Git credentials without changing global account settings.

Permission lists are scoped to each profile, not shared across accounts. For example, a personal profile can watch `pausegarra` while a work profile watches `ITACCIONA`; neither token searches the other profile's owners. Search failures remain isolated from unrelated accounts and workflows.

Existing global permission lists migrate when settings are loaded. Owners are copied to existing profiles without guessing which organizations each token can access; review each profile's owners once after upgrading. Authors and reviewers retain only the matching profile's login, preserving the previously effective account permissions. Explicit profile lists, including empty lists, take precedence. New profiles start with empty lists. Saved settings contain only profile scopes; token credentials and workflow activation preferences are preserved.

GitHub tokens are stored in macOS Keychain, Windows Credential Manager or Linux Secret Service. SQLite stores account metadata and opaque credential references. There is no plaintext fallback: unlock the system credential store if saving or reading a token fails. Linux needs a running user D-Bus session and an unlocked Secret Service provider (such as GNOME Keyring or KWallet). Each GitHub operation retrieves only its actor's credential; an unavailable profile does not disable the scheduler or unrelated accounts. Native credential reads run outside the async executor. Historical logs are withheld if required secrets cannot be loaded for redaction.

On first launch after upgrading, existing plaintext tokens are written to the native store and read back before settings are replaced. A failed migration keeps the previous settings available for retry. Successful migration scrubs the live SQLite database and truncates its WAL; it cannot erase external backups or filesystem snapshots. Token rotation uses a new verified entry before switching references. Failed cleanup is queued and retried on the next save or launch. Credentials are local to the OS account and do not travel with a copied SQLite file; re-enter tokens on another machine. The engine's `Credentials` port can be supplied by a server secret manager; tests use an injected in-memory store.

## Three triggers

- **New issue:** open issues with titles starting with `[AI]`, in configured owners, from allowed authors.
- **PR approved:** only PRs published and verified by an issue workflow. A review must approve the current head SHA. Alternatively, a new conversation comment exactly `OK` after trimming whitespace and ignoring case is accepted when its author is both the PR author and source issue author. A head creates at most one event; consumed comment IDs remain consumed across restarts and head changes.
- **Review requested:** open PRs with a request directed to a configured reviewer. After observing withdrawal, a fresh request creates another event. The PR need not originate from an issue workflow.

Each trigger has its own editable DAG and enabled switch. Nodes wait for successful dependencies; blocked or failed nodes prevent descendants from executing. Writers are serialized within each run. Per-flow node parallelism is configurable. The default global limit is **4 runs**, shared by all triggers; additional runs wait in SQLite. PR approved has an additional fixed limit of **one active worker per installation**, across repositories and accounts. Further approved PRs remain queued without preparing a workspace or starting agents. This slot remains occupied through GitHub actions, cancellation drain and workspace cleanup until the worker exits. New issue and PR review can still use other free global slots. No extra setting or workflow change is required.

Workflows provides a global switch and a preference for each GitHub profile for the selected trigger. New runs are allowed only when both are enabled. Profiles without a saved preference are allowed by default; the global switch always wins. Save workflow applies the draft; Discard restores saved settings. These controls affect this installation, not GitHub Actions. Activation uses the issue author for new issues, the source issue author for approved PRs, and the requested reviewer for reviews.

Preferences follow the stable profile ID, not its token value, account label or row position. Rotating a token preserves preferences. Removed profiles appear unavailable, with removable preferences; a new profile ID does not inherit an old exclusion. Activation still requires the existing allowlists, credentials, identity and permission checks.

Model menus are loaded from the selected installed CLI. Codex supplies its available models and reasoning efforts; OpenCode 2 supplies enabled provider models and their variants. Refresh reloads the catalog. Changing agent clears model and effort; changing model clears effort. Discovery failures keep saved values visible and allow retry. An empty model keeps the CLI default.

Dependencies are edited only on the canvas: drag a step’s output dot to another step’s input dot. Each connection is one dependency; select an arrow and press Delete to remove it. Parallel dependents display as separate branches. Arrange flow restores a dependency-based layout; manually moved node positions are preserved.

Runs keep immutable flow snapshots. Retrying creates a new attempt using the current flow. Uncertain sessions after an unexpected exit are interrupted and require an explicit retry.

Each execution gets a fresh clone in `workspaces/<run-id>` under the application data directory. Nodes in that execution share its clone; concurrent issues and retry attempts use different directories. PR workflows fetch and verify the expected head SHA. GitHub tokens stay in process-local Git configuration and are not embedded in remote URLs.

After all agent and Git processes stop, Factory removes the clone on success, failure or cancellation. Startup also removes leftovers belonging to known completed/interrupted runs; unrelated directories and queued runs are preserved. Cleanup failures remain visible and are retried at the next launch. Existing legacy local-folder and agent timeout settings are accepted but no longer used.

History, session logs, structured results and project memory stay outside temporary clones. Declared artifacts are validated, copied into persistent per-run memory, and returned with their archived paths and hashes so downstream steps and history retain them. Unpublished changes and undeclared files in a workspace are removed when the execution ends; workflows that implement code should include their publishing step before completion.

## Live operations

Overview and Runs show queued, active and historical tasks. Run details include step states, session identity, results and live logs, with cancel/retry actions. Rust emits Tauri events for creation, lifecycle changes and output. The UI subscribes before reading its initial snapshot and repeats reads if events arrive during a request. Event lag forces resynchronization.

**Delete run** permanently removes one stopped attempt and its local logs, results,
artifacts, private memory and remaining managed workspace. Cancel queued or running
runs first, then wait for their processes and cleanup to finish. Confirm deletion
from run details; there is no undo.

GitHub resources, other attempts, shared import archives, action receipts and
verified PR provenance are kept. Deleting the last attempt for an event clears its
deduplication mark, so eligible issues can run again on the next sync. While any
attempt remains, including pending deletions, the mark prevents duplicate runs.
Pending deletions show their run ID and a **Retry deletion**
action; restarting Factory also resumes cleanup, never the workflow. Local deletion
does not require a GitHub token or network access.

External source files, CLI-owned sessions, backups and copies already embedded in
other runs are outside this deletion. A later explicit legacy import can restore
imported history from its unchanged source archive.

Disabling a workflow applies to new runs and explicit retries, including Sync now and periodic discovery. A disabled retry returns an error without creating an attempt. Already queued or running tasks continue with their immutable snapshots, including their own GitHub actions and review correction rounds. Use Cancel to stop an existing task.

Disabled discovery skips the corresponding credential lookup and GitHub queries. It does not consume issues, approvals, OK comments or review request observations. Saving and final admission share a transaction boundary: if disabling is saved before admission, even an in-flight discovery response cannot consume the event. Read requests already in flight need not be cancelled. Reenabling discovers work that remains eligible; it does not replay closed work or reconstruct review withdrawal/rerequest transitions never observed while disabled. Existing deduplication and consumed approvals remain intact.

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

Artifact `kind` is descriptive metadata, not verification evidence. Factory replaces missing or blank categories with the neutral value `artifact`, and accepts a legacy `type` category when `kind` is absent. This normalization does not alter status, verification limitations, approval decisions or requested actions. Formatting repairs preserve the exact multiset of artifact paths and hashes and any originally declared categories; missing category metadata alone is not a workflow blocker. The canonical response must still pass the node schema and every artifact must pass filesystem and hash validation.

Agents request GitHub operations through `data`; the engine performs REST calls with the actor token:

- `publish: {title, body}` pushes the issue branch and creates its PR against the repository's current default branch, resolved through GitHub before pushing. Legacy `base` values from agents are ignored. An existing matching PR is reused without retargeting it. The body must close the source issue using `Closes #N`.
- `merge: {method: "squash"}` in an approved PR flow, guarded by its head SHA. After confirming the merge, Factory deletes only the verified remote source branch in the same repository, never the target or default branch.
- `review: {event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT", body}` in a review flow.
- `comment: "text"` comments on the event.

Factory validates every requested action before performing any mutation. Blocked results can report comments or request changes, but cannot publish, merge or approve. Retries reconcile existing PRs/merges and use durable receipts plus hidden markers for comments and reviews, including recovery after a lost API response. Cancellation is checked before new writes.

Source-branch cleanup is part of completing the existing merge action; saved workflows need no new action or opt-in. Factory rechecks PR identity, target, default branch and the source reference's approved SHA before deletion. Fork cleanup and local-branch deletion are not supported, and branch protections are never bypassed. An already absent source is accepted only after verifying repository and reference access. If merge succeeds but cleanup fails, the action reports a partial failure without undoing the merge or running later actions as if cleanup succeeded. Use the existing explicit retry to finish cleanup without repeating the merge. A source branch that has visibly moved is preserved. GitHub deletion has no conditional SHA guard, so a concurrent push between the final read and deletion remains a limitation.

Default flow prompts explain these contracts. No `gh` process is used by the desktop app.

Every node also receives a shared unattended execution instruction, including nodes in saved or custom workflows. Agents complete their assigned role without asking for human spec/plan approval, clarification or execution-mode choices: spec authors deliver the specification, planners deliver the plan, builders finish implementation/build/verification, and reviewers return a decision. Skills' interactive checkpoints are delegated to downstream workflow reviews. Nonblocking ambiguity is resolved with documented assumptions; genuine blockers return a structured `blocked` result instead of waiting for a reply. Access restrictions and actual upstream review decisions still apply. Codex uses `approval_policy="never"`; OpenCode runs automatically with its question tool denied.

Node skill selections apply to execution as part of the saved workflow snapshot. Omitted `skills` keeps native discovery (`Select all`); an explicit array exposes only those installed skill IDs, and `[]` disables skills for that node. OpenCode receives native skill permission rules. Codex enumerates its actual workspace catalog through `skills/list`, then receives per-process skill configuration overrides. Only selected names, descriptions and manifest paths enter the node prompt; skill bodies remain loaded on demand. Selections also apply to response repair attempts. A missing selected skill or unsupported Codex discovery fails the node before agent execution rather than falling back to unrestricted skills. These filters control native skill discovery and invocation; existing filesystem/tool access restrictions still define what the agent can read or execute.

## Local data and migration

All app-owned data lives in the OS application data directory:

- macOS: `~/Library/Application Support/dev.aifactory.desktop`
- Windows: `%APPDATA%` followed by `dev.aifactory.desktop`
- Ubuntu: `~/.local/share/dev.aifactory.desktop`, respecting `XDG_DATA_HOME`.

This includes `factory.sqlite3`, settings, temporary workspaces, persistent project memory, prompts, results and session logs. Settings shows the exact path. Factory clones directly from GitHub and does not modify local repositories.

Settings provides an explicit legacy YAML and/or SQLite import. The source database opens read-only. Available history, flow snapshots, logs, memory, PR provenance and trigger/comment/review deduplication are preserved. Legacy pending or active work is imported as interrupted for review before retrying. Imports are idempotent and report missing data with warnings and an archive of source tables. Tokens previously managed through `gh` must be added in Settings.

Old settings without profile activation preferences allow profiles under their existing global switch. Importing legacy configuration resets those preferences to this default while respecting the imported global setting. Importing only history preserves current preferences and existing snapshots; historical snapshots are never backfilled with current preferences.

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
