import { useEffect, useId, useRef, useState, type RefObject } from "react";
import { Plus } from "@phosphor-icons/react";
export type ConnectorKind = "telegram" | "google_chat";
export function ConnectorPicker({disabled, buttonRef, onSelect}: {
  disabled: boolean;
  buttonRef: RefObject<HTMLButtonElement | null>;
  onSelect: (kind: ConnectorKind) => void;
}) {
  const [open, setOpen] = useState(false);
  const select = useRef<HTMLSelectElement>(null);
  const id = useId();
  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  useEffect(() => { if (open) select.current?.focus(); }, [open]);
  function close() { setOpen(false); buttonRef.current?.focus(); }
  return <div className="notifications-picker" onBlur={e => {
    if (!e.currentTarget.contains(e.relatedTarget)) setOpen(false);
  }}>
    <button ref={buttonRef} className="button button-secondary" disabled={disabled} aria-expanded={open} aria-controls={open ? id : undefined} onClick={() => setOpen(value => !value)}><Plus size={16} aria-hidden="true" />Add connector</button>
    {open && !disabled && <select id={id} ref={select} aria-label="Connector to add" defaultValue="" onKeyDown={e => { if (e.key === "Escape") { e.preventDefault(); close(); } }} onChange={e => {
      const kind = e.target.value;
      if (kind === "telegram" || kind === "google_chat") { setOpen(false); onSelect(kind); }
    }}>
      <option value="" disabled>Choose a connector</option>
      <option value="telegram">Telegram</option>
      <option value="google_chat">Google Chat</option>
    </select>}
  </div>;
}
