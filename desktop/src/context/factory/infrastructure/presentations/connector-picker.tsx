import { useEffect, useId, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { createPortal } from "react-dom";
import { Plus } from "@phosphor-icons/react";
import { NotificationConnectorIcon } from "./notification-connector-icon";

export type ConnectorKind = "telegram" | "google_chat";
type ConnectorPickerProps = {
  disabled: boolean;
  buttonRef: RefObject<HTMLButtonElement | null>;
  onSelect: (kind: ConnectorKind) => void;
};
const providers: ReadonlyArray<{ kind: ConnectorKind; label: string }> = [
  { kind: "telegram", label: "Telegram" },
  { kind: "google_chat", label: "Google Chat" },
];

function ConnectorPickerDialog({ id, buttonRef, onCancel, onSelect }: {
  id: string;
  buttonRef: ConnectorPickerProps["buttonRef"];
  onCancel: () => void;
  onSelect: ConnectorPickerProps["onSelect"];
}) {
  const dialog = useRef<HTMLElement>(null);
  const close = useRef<HTMLButtonElement>(null);
  const restoreFocus = useRef(true);
  const selecting = useRef(false);
  useLayoutEffect(() => {
    const node = dialog.current;
    // Capture the opener before React clears its ref during unmount.
    const opener = buttonRef.current;
    close.current?.focus();
    const contain = (event: FocusEvent) => {
      if (event.target instanceof Node && !node?.contains(event.target)) close.current?.focus();
    };
    document.addEventListener("focusin", contain);
    return () => {
      document.removeEventListener("focusin", contain);
      queueMicrotask(() => {
        // StrictMode replays effects while the same dialog is still connected.
        if (node?.isConnected || !restoreFocus.current) return;
        const target = opener?.isConnected && !opener.disabled && !opener.closest("[inert]")
          ? opener : document.querySelector<HTMLElement>("main:not([inert])");
        target?.focus();
      });
    };
  }, [buttonRef]);
  function select(kind: ConnectorKind) {
    if (selecting.current) return;
    selecting.current = true;
    // The configuration dialog takes focus after selection.
    restoreFocus.current = false;
    onSelect(kind);
  }
  return createPortal(<div className="dialog-backdrop connector-picker-backdrop" onKeyDown={event => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onCancel(); return; }
    if (event.key !== "Tab") return;
    const buttons = Array.from(dialog.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
    const first = buttons[0], last = buttons.at(-1);
    if (!first || !last) { event.preventDefault(); return; }
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
  }}>
    <section ref={dialog} id={id} role="dialog" aria-modal="true" aria-labelledby={`${id}-title`}
      className="confirm-dialog connector-picker-dialog">
      <div className="connector-picker-heading">
        <h2 id={`${id}-title`}>Choose a connector</h2>
        <button ref={close} type="button" className="button button-secondary"
          aria-label="Close connector selector" onClick={onCancel}>Close</button>
      </div>
      <div className="connector-picker-options">
        {providers.map(provider => <button key={provider.kind} type="button" className="connector-picker-option"
          onClick={() => select(provider.kind)}>
          <NotificationConnectorIcon type={provider.kind} />
          <span>{provider.label}</span>
        </button>)}
      </div>
    </section>
  </div>, document.body);
}

export function ConnectorPicker({ disabled, buttonRef, onSelect }: ConnectorPickerProps) {
  const [open, setOpen] = useState(false);
  const id = useId();
  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  return <div className="notifications-picker">
    <button ref={buttonRef} type="button" className="button button-secondary" disabled={disabled}
      aria-haspopup="dialog" aria-expanded={open && !disabled} aria-controls={open && !disabled ? id : undefined}
      onClick={() => { if (!disabled) setOpen(true); }}>
      <Plus size={16} aria-hidden="true" />Add connector
    </button>
    {open && !disabled && <ConnectorPickerDialog id={id} buttonRef={buttonRef} onCancel={() => setOpen(false)}
      onSelect={kind => { if (disabled) return; setOpen(false); onSelect(kind); }} />}
  </div>;
}
