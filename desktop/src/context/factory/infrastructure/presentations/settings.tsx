import { useEffect, useId, useRef, useState } from "react";
import {
  FloppyDisk,
  Folder,
  Key,
  Plus,
  Sliders,
  Terminal,
  Trash,
} from "@phosphor-icons/react";
import type { Settings, TokenProfile } from "../../domain/types";
import { validateFlowActivation, validateSettings } from "../../domain/rules";
import { factoryApi } from "../ipc/factory-api";
import { LegacyImport } from "./legacy-import";
import { AgentsSettings } from "./agent-settings";
import { Field, Notice, Toggle } from "./ui";
import { noticeText } from "./notice-content";
type ScopeField = "owners" | "authors" | "reviewers";
const settingsTabs = [
  { id: "general", label: "General" },
  { id: "github", label: "GitHub accounts" },
  { id: "agents", label: "Agents" },
] as const;
function scopeLists(settings: Settings): Record<string, Record<ScopeField, string>> {
  return Object.fromEntries(settings.token_profiles.map((profile) => [profile.id, {
    owners: profile.owners.join(", "),
    authors: profile.authors.join(", "),
    reviewers: profile.reviewers.join(", "),
  }]));
}
function reconcileScopeLists(
  current: Record<string, Record<ScopeField, string>>,
  previous: Settings,
  incoming: Settings,
): Record<string, Record<ScopeField, string>> {
  const previousLists = scopeLists(previous);
  return Object.fromEntries(incoming.token_profiles.map((profile) => {
    const saved = {
      owners: profile.owners.join(", "),
      authors: profile.authors.join(", "),
      reviewers: profile.reviewers.join(", "),
    };
    const old = previousLists[profile.id];
    const local = current[profile.id];
    return [profile.id, {
      owners: old?.owners === saved.owners ? local?.owners ?? saved.owners : saved.owners,
      authors: old?.authors === saved.authors ? local?.authors ?? saved.authors : saved.authors,
      reviewers: old?.reviewers === saved.reviewers ? local?.reviewers ?? saved.reviewers : saved.reviewers,
    }];
  }));
}
export function SettingsPage({
  settings,
  onSave,
  appDir,
  onDirtyChange,
}: {
  settings: Settings;
  onSave: (s: Settings) => Promise<Settings | void>;
  appDir: string;
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const [draft, setDraft] = useState(() => structuredClone(settings));
  const [lists, setLists] = useState(() => scopeLists(settings));
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [accountBusy, setAccountBusy] = useState(false);
  const [tab, setTab] = useState<typeof settingsTabs[number]["id"]>("general");
  const tabsId = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  const mounted = useRef(true);
  const saveSerial = useRef(0);
  const importRefreshSerial = useRef(0);
  const editRevision = useRef(0);
  const previousSettings = useRef(settings);
  const dirty = JSON.stringify(draft) !== JSON.stringify(settings);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      saveSerial.current += 1;
      importRefreshSerial.current += 1;
    };
  }, []);
  useEffect(() => {
    const previous = previousSettings.current;
    const wasClean = JSON.stringify(draft) === JSON.stringify(previous);
    previousSettings.current = settings;
    if (wasClean && !busy) {
      setDraft(structuredClone(settings));
      setLists((current) => reconcileScopeLists(current, previous, settings));
    }
  }, [settings]);
  useEffect(() => {
    onDirtyChange?.(dirty);
    return () => onDirtyChange?.(false);
  }, [dirty, onDirtyChange]);
  function update(patch: Partial<Settings>) {
    editRevision.current += 1;
    setDraft((s) => ({ ...s, ...patch }));
    setError("");
    setSaved(false);
  }
  function updateProfile(id: string, patch: Partial<TokenProfile>) {
    editRevision.current += 1;
    setDraft((value) => ({
      ...value,
      token_profiles: value.token_profiles.map((profile) =>
        profile.id === id ? { ...profile, ...patch } : profile,
      ),
    }));
    setError("");
    setSaved(false);
  }
  function removeProfile(id: string) {
    editRevision.current += 1;
    setDraft((value) => ({
      ...value,
      token_profiles: value.token_profiles.filter((profile) => profile.id !== id),
      ...(value.codex_bindings ? { codex_bindings: Object.fromEntries(Object.entries(value.codex_bindings).filter(([profileId]) => profileId !== id)) } : {}),
    }));
    setError("");
    setSaved(false);
  }
  function removeAccountBindings(accountId: string) {
    editRevision.current += 1;
    setDraft((value) => ({
      ...value,
      ...(value.codex_bindings ? { codex_bindings: Object.fromEntries(Object.entries(value.codex_bindings).filter(([, id]) => id !== accountId)) } : {}),
    }));
    setError("");
    setSaved(false);
  }
  function updateScope(profile: TokenProfile, field: ScopeField, value: string) {
    setLists((previous) => ({
      ...previous,
      [profile.id]: { ...previous[profile.id], [field]: value },
    }));
    updateProfile(profile.id, { [field]: split(value) });
  }
  async function save() {
    if (busy || accountBusy) return;
    setSaved(false);
    setError("");
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
    const ticket = ++saveSerial.current;
    const revision = editRevision.current;
    const current = () => mounted.current && saveSerial.current === ticket;
    setBusy(true);
    try {
      const stored = await onSave(next) ?? next;
      const clean = {
        ...stored,
        token_profiles: stored.token_profiles.map((t) => ({
          ...t,
          has_token: t.has_token || !!t.token,
          token: undefined,
        })),
      };
      if (current() && editRevision.current === revision) {
        setDraft(clean);
        setLists(scopeLists(clean));
        setSaved(true);
      }
    } catch (e) {
      if (current()) setError(noticeText("Could not save settings.", e));
    } finally {
      if (current()) setBusy(false);
    }
  }
  function discard() {
    editRevision.current += 1;
    setDraft(structuredClone(settings));
    setLists(scopeLists(settings));
    setError("");
    setSaved(false);
  }
  async function imported() {
    const ticket = ++importRefreshSerial.current;
    const revision = editRevision.current;
    const latest = await factoryApi.settings();
    if (
      mounted.current &&
      importRefreshSerial.current === ticket &&
      editRevision.current === revision
    ) {
      setDraft(latest);
      setLists(scopeLists(latest));
      setSaved(false);
    }
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
          <h1 ref={heading} tabIndex={-1}>
            Settings
          </h1>
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
            disabled={busy || accountBusy || !dirty}
            onClick={() => void save()}
          >
            <FloppyDisk size={17} />
            {busy ? "Saving…" : "Save changes"}
          </button>
        </div>
      </div>
      {error && <Notice close={() => setError("")} focusAfterClose={() => heading.current}>{error}</Notice>}
      {saved && (
        <Notice tone="success" close={() => setSaved(false)} focusAfterClose={() => heading.current}>
          Settings saved.
        </Notice>
      )}
      <div className="segmented settings-tabs" role="tablist" aria-label="Settings sections">
        {settingsTabs.map(({ id, label }, index) => (
          <button
            key={id}
            id={`${tabsId}-${id}`}
            role="tab"
            aria-selected={tab === id}
            aria-controls={`${tabsId}-${id}-panel`}
            tabIndex={tab === id ? 0 : -1}
            className={tab === id ? "selected" : ""}
            onClick={() => setTab(id)}
            onKeyDown={(event) => {
              const next = event.key === "Home" ? settingsTabs[0].id
                : event.key === "End" ? settingsTabs[settingsTabs.length - 1].id
                : event.key === "ArrowLeft" ? settingsTabs[(index + settingsTabs.length - 1) % settingsTabs.length].id
                : event.key === "ArrowRight" ? settingsTabs[(index + 1) % settingsTabs.length].id
                : undefined;
              if (next) {
                event.preventDefault();
                setTab(next);
                document.getElementById(`${tabsId}-${next}`)?.focus();
              }
            }}
          >
            {label}
          </button>
        ))}
      </div>
      <div role="tabpanel" id={`${tabsId}-agents-panel`} aria-labelledby={`${tabsId}-agents`} hidden={tab !== "agents"}>
        {tab === "agents" && <AgentsSettings
            profiles={draft.token_profiles}
            bindings={draft.codex_bindings ?? {}}
            disabled={busy || accountBusy}
            onBusyChange={(value) => {
              if (mounted.current) setAccountBusy(value);
            }}
            onBindingChange={(profileId, accountId) => {
              if (!mounted.current) return;
              editRevision.current += 1;
              setDraft((value) => ({ ...value, codex_bindings: Object.fromEntries([
                ...Object.entries(value.codex_bindings ?? {}).filter(([id]) => id !== profileId),
                ...(accountId ? [[profileId, accountId]] : []),
              ]) }));
              setError("");
              setSaved(false);
            }}
            onAccountRemoved={(accountId) => {
              if (mounted.current) removeAccountBindings(accountId);
            }}
          />}
      </div>
      <div className="settings-stack" role="tabpanel" id={`${tabsId}-github-panel`} aria-labelledby={`${tabsId}-github`} hidden={tab !== "github"}>
        <section className="panel settings-section">
          <div className="section-intro">
            <div className="section-icon">
              <Key size={20} />
            </div>
            <div>
              <h2>GitHub accounts</h2>
              <p>
                One token per account, stored in your system credential store.
                Each account searches only its owners and uses its own permissions.
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
                      owners: [],
                      authors: [],
                      reviewers: [],
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
                <fieldset className="editable-row token-profile" key={profile.id}>
                  <legend className="sr-only">
                    {profile.label || `Account ${i + 1}`}{profile.account ? ` (@${profile.account})` : ""}
                  </legend>
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
                    onClick={() => removeProfile(profile.id)}
                  >
                    <Trash size={18} />
                  </button>
                  <div className="form-grid three-col profile-scopes">
                    <Field label="GitHub owners" hint="Organizations or users searched by this account. Separate with commas.">
                      <input
                        value={lists[profile.id]?.owners ?? profile.owners.join(", ")}
                        placeholder="your-organization"
                        autoCapitalize="none"
                        spellCheck={false}
                        onChange={(e) => updateScope(profile, "owners", e.target.value)}
                      />
                    </Field>
                    <Field label="Allowed authors" hint="Include this account to trigger issues and approvals.">
                      <input
                        value={lists[profile.id]?.authors ?? profile.authors.join(", ")}
                        placeholder="github-login"
                        autoCapitalize="none"
                        spellCheck={false}
                        onChange={(e) => updateScope(profile, "authors", e.target.value)}
                      />
                    </Field>
                    <Field label="Allowed reviewers" hint="Include this account to trigger requested reviews.">
                      <input
                        value={lists[profile.id]?.reviewers ?? profile.reviewers.join(", ")}
                        placeholder="github-login"
                        autoCapitalize="none"
                        spellCheck={false}
                        onChange={(e) => updateScope(profile, "reviewers", e.target.value)}
                      />
                    </Field>
                  </div>
                </fieldset>
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
      </div>
      <div className="settings-stack" role="tabpanel" id={`${tabsId}-general-panel`} aria-labelledby={`${tabsId}-general`} hidden={tab !== "general"}>
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
                Enter each installed CLI command or absolute path. Manage Codex
                accounts in Agents; OpenCode uses its default CLI account.
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
              checked={draft.codex_yolo ?? false}
              onChange={(checked) => update({ codex_yolo: checked })}
              label="Codex YOLO mode"
              description="Run all Codex steps without sandboxing or approval prompts, with full filesystem and network access. macOS permissions still apply."
            />
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
        <LegacyImport disabled={dirty} onImported={imported} active={tab === "general"} />
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
