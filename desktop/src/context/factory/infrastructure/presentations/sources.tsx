import { useEffect, useId, useRef, useState } from "react";
import { Plugs } from "@phosphor-icons/react";
import type { SourceConnector, Settings } from "../../domain/types";
import { cleanSourceConnectors, newGitHubIssuesSource, normalizePullRequestAuthors, validateSourceConnectors } from "../../domain/source-connectors";
import { Empty, Notice } from "./ui";
import { ConfiguredConnectorPicker } from "./connector-picker";
import { SourceConnectorDialog } from "./source-connector-dialog";
import { SourceConnectorIcon } from "./source-connector-icon";

type ConnectorEditSession = {
  mode: "new" | "existing";
  original: SourceConnector;
  value: SourceConnector;
};

// Refresh public credential presence without touching pending replacement secrets.
function refreshCredentialHints(items: SourceConnector[], incoming: SourceConnector[]): SourceConnector[] {
  return items.map(c => ({...c, has_token:incoming.some(saved => saved.id === c.id && saved.account.toLowerCase() === c.account.toLowerCase() && saved.has_token)}));
}

export function SourcesPage({ settings, onSave, onDirtyChange, onBusyChange }: {
  settings: Settings;
  onSave: (settings: Settings) => Promise<Settings | void>;
  onDirtyChange?: (dirty: boolean) => void;
  onBusyChange?: (busy: boolean) => void;
}) {
  const publicConnectors = () => cleanSourceConnectors(settings.source_connectors ?? []);
  const [baseline, setBaseline] = useState(publicConnectors);
  const [draft, setDraft] = useState(publicConnectors);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const mounted = useRef(true);
  const inFlight = useRef(false);
  const serial = useRef(0);
  const lastProps = useRef(JSON.stringify(publicConnectors()));
  const addButton = useRef<HTMLButtonElement>(null);
  const rows = useRef(new Map<string, HTMLButtonElement>());
  const [session, setSession] = useState<ConnectorEditSession | null>(null);
  const dialogId = useId();
  const deferredIncoming = useRef<SourceConnector[] | null>(null);
  const focusRow = useRef<string | null>(null);
  const dirty = JSON.stringify(draft) !== JSON.stringify(baseline);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; serial.current += 1; };
  }, []);
  useEffect(() => {
    const incoming = publicConnectors();
    const key = JSON.stringify(incoming);
    if (lastProps.current === key) return;
    lastProps.current = key;
    if (inFlight.current) return;
    const sessionChanged = session && JSON.stringify(session.value) !== JSON.stringify(session.original);
    if (!dirty && !sessionChanged) {
      setBaseline(incoming); setDraft(incoming); deferredIncoming.current = null;
      if (session?.mode === "existing") {
        const updated = incoming.find(c => c.id === session.value.id);
        setSession(updated ? {mode: "existing", original: structuredClone(updated), value: structuredClone(updated)} : null);
      }
    } else {
      if (!dirty) deferredIncoming.current = incoming;
      setBaseline(items => refreshCredentialHints(items, incoming));
      setDraft(items => refreshCredentialHints(items, incoming));
      setSession(current => current ? {...current,
        original: refreshCredentialHints([current.original], incoming)[0],
        value: refreshCredentialHints([current.value], incoming)[0],
      } : null);
    }
  }, [settings]);
  useEffect(() => { onDirtyChange?.(dirty); return () => onDirtyChange?.(false); }, [dirty, onDirtyChange]);
  useEffect(() => { onBusyChange?.(busy); return () => onBusyChange?.(false); }, [busy, onBusyChange]);
  function openExisting(c: SourceConnector) {
    if (inFlight.current || session) return;
    focusRow.current = c.id;
    setSession({mode: "existing", original: structuredClone(c), value: structuredClone(c)});
  }
  function add() {
    if (inFlight.current || session) return;
    const id = crypto.randomUUID();
    const c = newGitHubIssuesSource(id);
    focusRow.current = null;
    setSession({mode: "new", original: structuredClone(c), value: structuredClone(c)});
  }
  function editSession(patch: Partial<SourceConnector>) {
    if (inFlight.current) return;
    setSession(current => current ? {...current, value: {...current.value, ...patch}} : null);
  }
  function applySession() {
    if (!session || inFlight.current) return;
    const value = structuredClone(session.value);
    setDraft(items => session.mode === "new" ? [...items, value] : items.map(c => c.id === value.id ? value : c));
    setError(""); setSaved(false); deferredIncoming.current = null; setSession(null);
  }
  function cancelSession() {
    if (inFlight.current) return;
    if (!dirty && deferredIncoming.current) {
      setBaseline(structuredClone(deferredIncoming.current)); setDraft(structuredClone(deferredIncoming.current));
    }
    deferredIncoming.current = null; setSession(null);
  }
  function remove(id: string) {
    if (inFlight.current) return;
    const remaining = draft.filter(c => c.id !== id);
    const index = draft.findIndex(c => c.id === id);
    focusRow.current = remaining[Math.min(index, remaining.length - 1)]?.id ?? null;
    setDraft(remaining); setSaved(false); setError(""); deferredIncoming.current = null; setSession(null);
  }
  function discard() {
    if (inFlight.current) return;
    const clean = publicConnectors(); setBaseline(clean); setDraft(clean);
    deferredIncoming.current = null; setSession(null);
    setError(""); setSaved(false); setAttempted(false);
  }
  async function save() {
    if (inFlight.current || session) return;
    const connectors = structuredClone(draft).map(c => {
      c.has_token = baseline.some(saved => saved.id === c.id && saved.account.toLowerCase() === c.account.trim().toLowerCase() && saved.has_token);
      c.label = c.label.trim(); c.account = c.account.trim().toLowerCase();
      if (typeof c.issue_title_prefix === "string") c.issue_title_prefix = c.issue_title_prefix.trim();
      if (typeof c.release_issue_title_prefix === "string") c.release_issue_title_prefix = c.release_issue_title_prefix.trim();
      for (const field of ["owners", "authors", "reviewers"] as const) c[field] = [...new Set(c[field].map(v => v.trim().toLowerCase()))];
      c.pull_request_authors = normalizePullRequestAuthors(c.pull_request_authors ?? []);
      const token = c.token?.trim(); if (token) c.token=token; else delete c.token;
      return c;
    });
    setAttempted(true); setSaved(false);
    const errors = validateSourceConnectors(connectors);
    if (errors.length) {
      setError(errors.join(" "));
      const invalid = draft.find(c => validateSourceConnectors([c]).length);
      if (invalid) openExisting(invalid);
      return;
    }
    const ticket = ++serial.current;
    inFlight.current = true; setBusy(true); setError("");
    const current = () => mounted.current && ticket === serial.current;
    try {
      const next = {...settings, source_connectors: connectors};
      const result = await onSave(next);
      const stored = result?.source_connectors ?? connectors;
      const clean = cleanSourceConnectors(stored.map(c => ({...c, has_token:c.has_token || !!c.token})));
      if (current()) { deferredIncoming.current = null; setDraft(clean); setBaseline(clean); setSaved(true); setAttempted(false); }
    } catch {
      if (current()) setError("Could not save source settings. Check your configuration and try again.");
    } finally {
      if (current()) { inFlight.current = false; setBusy(false); }
    }
  }
  return <><section inert={!!session} className="notifications-page" aria-labelledby="sources-title">
    <div className="page-title">
      <div><h1 id="sources-title">Sources</h1><p>Configure where Factory discovers work.</p></div>
      <ConfiguredConnectorPicker disabled={busy || !!session} buttonRef={addButton} onSelect={add} options={[{kind:"github_issues",label:"GitHub Issues",icon:<SourceConnectorIcon />}]} />
    </div>
    <div className="notifications-content">
      {error && <Notice>{error}</Notice>}
      {saved && <Notice tone="success">Source settings saved.</Notice>}
      {!draft.length ? <Empty icon={<Plugs size={28} />} title="No source connectors" description="Add a GitHub Issues connector to discover work." /> :
        <ul className="notifications-connector-list" aria-label="Source connectors">
          {draft.map(c => <li key={c.id}>
            <button type="button" className="notifications-connector-row"
              ref={element => { if (element) rows.current.set(c.id, element); else rows.current.delete(c.id); }}
              disabled={busy || !!session} aria-label={`Edit connector ${c.label || "Connector"}`}
              aria-haspopup="dialog" aria-expanded={session?.value.id === c.id}
              aria-controls={session?.value.id === c.id ? dialogId : undefined} onClick={() => openExisting(c)}>
              <SourceConnectorIcon /><span>{c.label || "Connector"}</span>
            </button>
          </li>)}
        </ul>}
      <div className="notifications-form-actions" role="group" aria-label="Source settings actions">
        <button className="button button-primary" disabled={!dirty || busy || !!session} onClick={() => void save()}>{busy ? "Saving…" : "Save changes"}</button>
        <button className="button button-secondary" disabled={!dirty || busy || !!session} onClick={discard}>Discard changes</button>
      </div>
    </div>
  </section>
    {session && <SourceConnectorDialog id={dialogId} connector={session.value} mode={session.mode}
      attempted={attempted} busy={busy} onChange={editSession}
      onApply={applySession} onCancel={cancelSession}
      onRemove={session.mode === "existing" ? () => remove(session.value.id) : undefined}
      returnFocus={() => (focusRow.current ? rows.current.get(focusRow.current) : null) ?? addButton.current} />}
  </>;
}
