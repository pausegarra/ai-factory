import { useEffect, useState } from "react";
import {
  FloppyDisk,
  Folder,
  GitBranch,
  Key,
  Plus,
  Sliders,
  Terminal,
  Trash,
} from "@phosphor-icons/react";
import type { Settings } from "../../domain/types";
import { validateFlowActivation, validateSettings } from "../../domain/rules";
import { factoryApi } from "../ipc/factory-api";
import { LegacyImport } from "./legacy-import";
import { Field, Notice, Toggle } from "./ui";
export function SettingsPage({
  settings,
  onSave,
  appDir,
  onDirtyChange,
}: {
  settings: Settings;
  onSave: (s: Settings) => Promise<void>;
  appDir: string;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const [draft, setDraft] = useState(() => structuredClone(settings));
  const [lists, setLists] = useState({
    owners: settings.owners.join(", "),
    authors: settings.authors.join(", "),
    reviewers: settings.reviewers.join(", "),
  });
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const dirty = JSON.stringify(draft) !== JSON.stringify(settings);
  useEffect(() => {
    onDirtyChange?.(dirty);
    return () => onDirtyChange?.(false);
  }, [dirty, onDirtyChange]);
  function update(patch: Partial<Settings>) {
    setDraft((s) => ({ ...s, ...patch }));
    setSaved(false);
  }
  async function save() {
    const token_profiles = draft.token_profiles.map((profile) => ({
      ...profile,
      has_token: settings.token_profiles.some((saved) => saved.id === profile.id && saved.account.toLowerCase() === profile.account.toLowerCase() && saved.has_token),
      token: profile.token?.trim() || undefined,
    }));
    const next = { ...draft, token_profiles, flows: settings.flows };
    const errors = [...validateSettings(next), ...next.flows.flatMap(validateFlowActivation)];
    if (errors.length) {
      setError(errors.join(" "));
      return;
    }
    setBusy(true);
    setError("");
    try {
      await onSave(next);
      setDraft({
        ...next,
        token_profiles: next.token_profiles.map((t) => ({
          ...t,
          has_token: t.has_token || !!t.token,
          token: undefined,
        })),
      });
      setSaved(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  function discard() {
    setDraft(structuredClone(settings));
    setLists({
      owners: settings.owners.join(", "),
      authors: settings.authors.join(", "),
      reviewers: settings.reviewers.join(", "),
    });
    setError("");
    setSaved(false);
  }
  async function imported() {
    const latest = await factoryApi.settings();
    setDraft(latest);
    setLists({
      owners: latest.owners.join(", "),
      authors: latest.authors.join(", "),
      reviewers: latest.reviewers.join(", "),
    });
    setSaved(false);
  }
  const split = (value: string) =>
    value
      .split(/[\s,]+/)
      .map((v) => v.trim())
      .filter(Boolean);
  return (
    <>
      <div className="page-title">
        <div>
          <span className="eyebrow">YOUR FACTORY ENGINE</span>
          <h1>Settings</h1>
          <p>Connect GitHub accounts. Shape how your team works.</p>
        </div>
        <div className="flex gap-3">
          {dirty && (
            <button
              className="button button-secondary"
              disabled={busy}
              onClick={discard}
            >
              Discard
            </button>
          )}
          <button
            className="button button-primary"
            disabled={busy || !dirty}
            onClick={() => void save()}
          >
            <FloppyDisk size={17} />
            {busy ? "Saving…" : "Save changes"}
          </button>
        </div>
      </div>
      {error && <Notice close={() => setError("")}>{error}</Notice>}
      {saved && (
        <Notice tone="success" close={() => setSaved(false)}>
          Settings saved.
        </Notice>
      )}
      <div className="settings-stack">
        <section className="panel settings-section">
          <div className="section-intro">
            <div className="section-icon">
              <Key size={20} />
            </div>
            <div>
              <h2>GitHub accounts</h2>
              <p>
                One token per account, stored in your system credential store.
                AI Factory uses the event author’s profile.
              </p>
            </div>
            <button
              className="button button-secondary"
              onClick={() =>
                update({
                  token_profiles: [
                    ...draft.token_profiles,
                    {
                      id: crypto.randomUUID(),
                      label: "",
                      account: "",
                      has_token: false,
                      token: "",
                    },
                  ],
                })
              }
            >
              <Plus size={16} />
              Add account
            </button>
          </div>
          {draft.token_profiles.length ? (
            <div className="token-list">
              {draft.token_profiles.map((profile, i) => (
                <div className="editable-row" key={profile.id}>
                  <span className="row-index">
                    {String(i + 1).padStart(2, "0")}
                  </span>
                  <Field label="Profile name">
                    <input
                      placeholder="Personal or work"
                      value={profile.label}
                      onChange={(e) =>
                        update({
                          token_profiles: draft.token_profiles.map((t) =>
                            t.id === profile.id
                              ? { ...t, label: e.target.value }
                              : t,
                          ),
                        })
                      }
                    />
                  </Field>
                  <Field label="GitHub account">
                    <input
                      placeholder="github-login"
                      autoCapitalize="none"
                      spellCheck={false}
                      value={profile.account}
                      onChange={(e) =>
                        update({
                          token_profiles: draft.token_profiles.map((t) =>
                            t.id === profile.id
                              ? { ...t, account: e.target.value }
                              : t,
                          ),
                        })
                      }
                    />
                  </Field>
                  <Field
                    label={profile.has_token ? "Replace token" : "API token"}
                    hint={
                      profile.has_token
                        ? "Token saved. Leave blank to keep it."
                        : "Stored on this device only."
                    }
                  >
                    <input
                      type="password"
                      autoComplete="new-password"
                      placeholder={
                        profile.has_token ? "••••••••••••••••" : "github_pat_…"
                      }
                      value={profile.token || ""}
                      onChange={(e) =>
                        update({
                          token_profiles: draft.token_profiles.map((t) =>
                            t.id === profile.id
                              ? { ...t, token: e.target.value }
                              : t,
                          ),
                        })
                      }
                    />
                  </Field>
                  <button
                    className="icon-button danger"
                    aria-label={`Delete profile ${profile.label || i + 1}`}
                    onClick={() =>
                      update({
                        token_profiles: draft.token_profiles.filter(
                          (t) => t.id !== profile.id,
                        ),
                      })
                    }
                  >
                    <Trash size={18} />
                  </button>
                </div>
              ))}
            </div>
          ) : (
            <div className="inline-empty">
              <Key size={24} />
              <div>
                <strong>Connect your first account</strong>
                <p>
                  Add an API token with repository access to detect tasks and
                  publish results.
                </p>
              </div>
            </div>
          )}
          <div className="section-note">
            Tokens stay hidden after saving. Multiple accounts are supported.
          </div>
        </section>
        <section className="panel settings-section">
          <div className="section-intro">
            <div className="section-icon">
              <GitBranch size={20} />
            </div>
            <div>
              <h2>Permissions and scope</h2>
              <p>
                Choose which repositories to watch and who can trigger your
                agents.
              </p>
            </div>
          </div>
          <div className="form-grid three-col">
            <Field
              label="GitHub owners"
              hint="Organizations or users. Separate with commas."
            >
              <input
                value={lists.owners}
                placeholder="your-organization"
                onChange={(e) => {
                  setLists((v) => ({ ...v, owners: e.target.value }));
                  update({ owners: split(e.target.value) });
                }}
              />
            </Field>
            <Field
              label="Allowed authors"
              hint="Authors of issues and approvals."
            >
              <input
                value={lists.authors}
                placeholder="github-login"
                onChange={(e) => {
                  setLists((v) => ({ ...v, authors: e.target.value }));
                  update({ authors: split(e.target.value) });
                }}
              />
            </Field>
            <Field
              label="Allowed reviewers"
              hint="Accounts receiving review requests."
            >
              <input
                value={lists.reviewers}
                placeholder="github-login"
                onChange={(e) => {
                  setLists((v) => ({ ...v, reviewers: e.target.value }));
                  update({ reviewers: split(e.target.value) });
                }}
              />
            </Field>
          </div>
        </section>
        <section className="panel settings-section">
          <div className="section-intro">
            <div className="section-icon">
              <Folder size={20} />
            </div>
            <div>
              <h2>Temporary workspaces</h2>
              <p>
                Repositories are cloned automatically from GitHub for each run.
                No local repository setup is needed.
              </p>
            </div>
          </div>
          <div className="section-note">
            <code>{appDir.replace(/[\\/]+$/, "")}/workspaces/&lt;run-id&gt;</code>
            <p>
              Workspaces are deleted after success, failure, or cancellation.
              Logs, results, and declared artifacts are kept outside temporary
              workspaces.
            </p>
          </div>
        </section>
        <section className="panel settings-section">
          <div className="section-intro">
            <div className="section-icon">
              <Terminal size={20} />
            </div>
            <div>
              <h2>Agents and tools</h2>
              <p>
                Install and authenticate your CLIs, then enter their command or
                absolute path.
              </p>
            </div>
          </div>
          <div className="form-grid three-col">
            {(["codex", "opencode", "git"] as const).map((agent) => (
              <Field
                key={agent}
                label={
                  agent === "codex"
                    ? "Codex CLI"
                    : agent === "opencode"
                      ? "OpenCode CLI"
                      : "Git"
                }
              >
                <input
                  className="code-input"
                  spellCheck={false}
                  value={draft.commands[agent]}
                  onChange={(e) =>
                    update({
                      commands: { ...draft.commands, [agent]: e.target.value },
                    })
                  }
                />
              </Field>
            ))}
          </div>
        </section>
        <section className="panel settings-section">
          <div className="section-intro">
            <div className="section-icon">
              <Sliders size={20} />
            </div>
            <div>
              <h2>Execution</h2>
              <p>
                All three triggers share this capacity. Extra tasks wait in the
                queue.
              </p>
            </div>
          </div>
          <div className="form-grid">
            <Field label="Concurrent tasks" hint="From 1 to 64. Defaults to 4.">
              <input
                type="number"
                min={1}
                max={64}
                step={1}
                value={draft.max_concurrent}
                onChange={(e) =>
                  update({ max_concurrent: Number(e.target.value) })
                }
              />
            </Field>
            <Field
              label="GitHub poll interval · seconds"
              hint="Time between event checks."
            >
              <input
                type="number"
                min={1}
                step={1}
                value={draft.poll_interval_seconds}
                onChange={(e) =>
                  update({ poll_interval_seconds: Number(e.target.value) })
                }
              />
            </Field>
          </div>
          <div className="startup-setting">
            <Toggle
              checked={draft.start_at_login}
              onChange={(checked) => update({ start_at_login: checked })}
              label="Launch at login"
              description="AI Factory will be ready when you sign in."
            />
          </div>
        </section>
        <LegacyImport disabled={dirty} onImported={imported} />
        <section className="storage-note">
          <Folder size={20} />
          <div>
            <h3>Your data stays local</h3>
            <p>
              SQLite, settings, memory, sessions and workspaces live in the
              application data directory.
            </p>
            <code>{appDir}</code>
          </div>
        </section>
      </div>
    </>
  );
}
