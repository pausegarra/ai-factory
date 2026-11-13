import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowSquareOut, ArrowsClockwise, Plus, Robot, Terminal, Trash } from "@phosphor-icons/react";
import type { AgentAccount, TokenProfile } from "../../domain/types";
import { factoryApi, native } from "../ipc/factory-api";
import { Field, Notice } from "./ui";
import { noticeText } from "./notice-content";

interface AgentsSettingsProps {
  profiles: TokenProfile[];
  bindings: Record<string, string>;
  disabled: boolean;
  onBindingChange: (profileId: string, accountId: string) => void;
  onAccountRemoved: (accountId: string) => void;
  onBusyChange: (busy: boolean) => void;
}

export function AgentsSettings(props: AgentsSettingsProps) {
  return (
    <div className="settings-stack">
      <CodexAccounts {...props} />
      <section className="panel settings-section" aria-labelledby="opencode-settings-heading">
        <div className="section-intro">
          <div className="section-icon"><Terminal size={20} /></div>
          <div>
            <h2 id="opencode-settings-heading">OpenCode</h2>
            <p>Uses the account and provider configured in your OpenCode CLI.</p>
          </div>
          <span className="agent-auth-mode">Default CLI</span>
        </div>
        <p className="section-note">Manage OpenCode authentication in your terminal.</p>
      </section>
    </div>
  );
}

function CodexAccounts({ profiles, bindings, disabled, onBindingChange, onAccountRemoved, onBusyChange }: AgentsSettingsProps) {
  const [accounts, setAccounts] = useState<AgentAccount[]>([]);
  const [label, setLabel] = useState("");
  const [loading, setLoading] = useState(true);
  const [mutating, setMutating] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [actionError, setActionError] = useState("");
  const heading = useRef<HTMLHeadingElement>(null);
  const alive = useRef(false);
  const revision = useRef(0);
  const inFlight = useRef(false);
  const accountMutation = useRef(false);
  const reloadSerial = useRef(0);
  const reloadRequested = useRef(false);

  const reload = useCallback(async () => {
    if (accountMutation.current) return;
    if (inFlight.current) {
      reloadRequested.current = true;
      return;
    }
    inFlight.current = true;
    reloadRequested.current = false;
    const ticket = ++reloadSerial.current;
    const currentRevision = revision.current;
    const current = () => alive.current && ticket === reloadSerial.current;
    if (alive.current) {
      setLoading(true);
      setLoadError("");
    }
    try {
      const result = await factoryApi.agentAccounts();
      if (current() && currentRevision === revision.current) {
        setAccounts(result);
        setLoadError("");
      }
    } catch (reason) {
      if (current() && currentRevision === revision.current) {
        setLoadError(noticeText("Could not load Codex accounts.", reason));
      }
    } finally {
      inFlight.current = false;
      if (current()) setLoading(false);
      if (reloadRequested.current && alive.current && !accountMutation.current) {
        void reload();
      }
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    void reload();
    return () => {
      alive.current = false;
      reloadSerial.current += 1;
    };
  }, [reload]);

  const pending = accounts.some((account) => account.status === "pending");
  useEffect(() => {
    if (!pending || mutating) return;
    const timer = window.setInterval(() => { void reload(); }, 1500);
    return () => window.clearInterval(timer);
  }, [pending, mutating, reload]);

  async function mutate(
    action: (setFailureTitle: (title: string) => void) => Promise<void>,
    initialFailureTitle: string,
  ) {
    if (accountMutation.current || disabled) return;
    accountMutation.current = true;
    revision.current += 1;
    const currentRevision = revision.current;
    let failureTitle = initialFailureTitle;
    const setFailureTitle = (title: string) => { failureTitle = title; };
    setMutating(true);
    onBusyChange(true);
    setActionError("");
    try {
      await action(setFailureTitle);
    } catch (reason) {
      if (alive.current && currentRevision === revision.current) {
        setActionError(noticeText(failureTitle, reason));
      }
    } finally {
      accountMutation.current = false;
      onBusyChange(false);
      if (alive.current && currentRevision === revision.current) setMutating(false);
    }
  }

  async function addAccount() {
    const name = label.trim();
    if (!name) return;
    await mutate(async (setFailureTitle) => {
      const login = await factoryApi.startCodexLogin(name);
      if (alive.current) {
        setAccounts((previous) => [...previous.filter((account) => account.id !== login.account.id), login.account]);
        setLabel("");
      }
      setFailureTitle("Could not continue Codex sign-in.");
      await factoryApi.openCodexLogin(login.account.id);
    }, "Could not add Codex account.");
  }

  async function removeAccount(accountId: string) {
    await mutate(async () => {
      await factoryApi.removeAgentAccount(accountId);
      onAccountRemoved(accountId);
      if (alive.current) setAccounts((previous) => previous.filter((account) => account.id !== accountId));
    }, "Could not remove Codex account.");
  }

  const readyAccounts = accounts.filter((account) => account.status === "ready");
  const locked = disabled || mutating;
  return (
    <section className="panel settings-section" aria-labelledby="codex-settings-heading">
      <div className="section-intro">
        <div className="section-icon"><Robot size={20} /></div>
        <div>
          <h2 id="codex-settings-heading" ref={heading} tabIndex={-1}>
            Codex
          </h2>
          <p>Connect Codex accounts with OAuth. Assign an account to each GitHub profile.</p>
        </div>
        <button type="button" className="button button-secondary" disabled={loading || locked || !native} onClick={() => void reload()}>
          <ArrowsClockwise size={16} />Reload accounts
        </button>
      </div>
      {!native && <Notice tone="info">Open AI Factory desktop to connect Codex accounts. OAuth is unavailable in browser preview.</Notice>}
      {loadError && (
        <Notice close={() => setLoadError("")} focusAfterClose={() => heading.current}>
          {loadError}
        </Notice>
      )}
      {actionError && (
        <Notice close={() => setActionError("")} focusAfterClose={() => heading.current}>
          {actionError}
        </Notice>
      )}
      <form className="agent-account-form" onSubmit={(event) => { event.preventDefault(); void addAccount(); }}>
        <Field label="Account label" hint="Choose a name to distinguish your Codex accounts.">
          <input value={label} onChange={(event) => setLabel(event.target.value)} placeholder="Personal or work" maxLength={100} disabled={locked || !native} />
        </Field>
        <button className="button button-secondary" disabled={locked || !native || !label.trim()} type="submit">
          <Plus size={16} />{mutating ? "Working…" : "Add Codex account"}
        </button>
      </form>
      <div className="agent-account-list" aria-busy={loading} aria-label="Codex accounts">
        {loading && !accounts.length ? (
          <div className="agent-account-loading" role="status"><span className="skeleton" />Loading Codex accounts…</div>
        ) : accounts.length ? accounts.map((account) => (
          <article className="agent-account" key={account.id} aria-label={account.label}>
            <div className="agent-account-identity">
              <strong>{account.label}</strong>
              {account.email && <span>{account.email}</span>}
              {account.status === "pending" && <small>Complete sign-in in your browser. Status updates automatically.</small>}
              {account.error && <p role="alert" className="agent-account-error">{account.error}</p>}
            </div>
            <span className={`status status-${account.status === "ready" ? "succeeded" : account.status}`}><span />{account.status === "ready" ? "Ready" : account.status === "pending" ? "Pending" : "Failed"}</span>
            <div className="agent-account-actions">
              {account.status === "pending" && <button type="button" className="button button-secondary" disabled={locked} onClick={() => void mutate(() => factoryApi.openCodexLogin(account.id), "Could not continue Codex sign-in.")}><ArrowSquareOut size={16} />Continue sign-in</button>}
              <button type="button" className="icon-button danger" disabled={locked} aria-label={account.status === "pending" ? `Cancel sign-in for ${account.label}` : `Remove account ${account.label}`} onClick={() => void removeAccount(account.id)}><Trash size={18} /></button>
            </div>
          </article>
        )) : (
          <div className="inline-empty"><Robot size={24} /><div><strong>No Codex accounts yet</strong><p>Add an account, then finish sign-in in your browser.</p></div></div>
        )}
      </div>
      <div className="agent-profile-bindings">
        <h3>GitHub profile assignments</h3>
        <p className="section-note">Unassigned GitHub profiles block Codex steps. Assignments apply after Save changes.</p>
        {profiles.length ? (
          <div className="form-grid">
            {profiles.map((profile) => {
              const accountId = Object.hasOwn(bindings, profile.id) ? bindings[profile.id] : "";
              const unavailable = accountId && !readyAccounts.some((account) => account.id === accountId);
              return (
                <Field key={profile.id} label={`Codex account for ${profile.label || "Unnamed profile"}${profile.account ? ` (@${profile.account})` : ""}`} hint={unavailable ? "Selected account is not ready. Codex steps are blocked." : undefined}>
                  <select value={accountId} disabled={locked || loading || !native} onChange={(event) => onBindingChange(profile.id, event.target.value)}>
                    <option value="">Unassigned · blocks Codex</option>
                    {unavailable && <option value={accountId} disabled>Unavailable account</option>}
                    {readyAccounts.map((account) => <option key={account.id} value={account.id}>{account.label}{account.email ? ` (${account.email})` : ""}</option>)}
                  </select>
                </Field>
              );
            })}
          </div>
        ) : <p className="section-note">Add profiles in GitHub accounts to assign Codex accounts.</p>}
      </div>
      <p className="section-note">Account connections and removals take effect immediately. Credentials stay on this device and never appear here.</p>
    </section>
  );
}
