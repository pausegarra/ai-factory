import {
  cloneElement,
  isValidElement,
  useId,
  type ReactNode,
  type ReactElement,
} from "react";
import { ArrowRight, Check, WarningCircle, X } from "@phosphor-icons/react";
export const STATUS: Record<string, string> = {
  queued: "Queued",
  running: "Running",
  succeeded: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
  interrupted: "Interrupted",
  pending: "Pending",
  skipped: "Skipped",
  blocked: "Blocked",
};
export function Badge({ status }: { status: string }) {
  return (
    <span className={`status status-${status}`}>
      <span />
      {STATUS[status] || status}
    </span>
  );
}
export function Notice({
  children,
  tone = "error",
  close,
}: {
  children: ReactNode;
  tone?: "error" | "success" | "info";
  close?: () => void;
}) {
  return (
    <div
      role={tone === "error" ? "alert" : "status"}
      className={`notice notice-${tone}`}
    >
      {tone === "success" ? <Check size={18} /> : <WarningCircle size={18} />}
      <span>{children}</span>
      {close && (
        <button
          className="icon-button"
          onClick={close}
          aria-label="Dismiss notice"
        >
          <X size={16} />
        </button>
      )}
    </div>
  );
}
export function Empty({
  icon,
  title,
  description,
  action,
  onAction,
}: {
  icon: ReactNode;
  title: string;
  description: string;
  action?: string;
  onAction?: () => void;
}) {
  return (
    <div className="empty-state">
      <div className="empty-icon">{icon}</div>
      <h3>{title}</h3>
      <p>{description}</p>
      {action && (
        <button className="button button-secondary mt-6" onClick={onAction}>
          {action}
          <ArrowRight size={16} />
        </button>
      )}
    </div>
  );
}
export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  const id = useId();
  const control =
    isValidElement(children) &&
    typeof children.type === "string" &&
    ["input", "select", "textarea"].includes(children.type)
      ? cloneElement(children as ReactElement<Record<string, unknown>>, {
          "aria-labelledby": id,
          "aria-describedby": hint ? `${id}-hint` : undefined,
        })
      : children;
  return (
    <div className="field">
      <span id={id}>{label}</span>
      {control}
      {hint && <small id={`${id}-hint`}>{hint}</small>}
    </div>
  );
}
export function Toggle({
  checked,
  onChange,
  label,
  description,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  description?: string;
}) {
  return (
    <label className="toggle-row">
      <span>
        <strong>{label}</strong>
        {description && <small>{description}</small>}
      </span>
      <input
        type="checkbox"
        className="sr-only"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className={`toggle ${checked ? "checked" : ""}`} aria-hidden="true">
        <span />
      </span>
    </label>
  );
}
export function date(value: string | null, short = false) {
  if (!value) return "—";
  const d = new Date(value);
  return Number.isNaN(d.getTime())
    ? value
    : new Intl.DateTimeFormat("en-US", {
        day: "2-digit",
        month: "short",
        ...(short ? {} : { hour: "2-digit", minute: "2-digit" }),
      }).format(d);
}
export function duration(start: string | null, end: string | null) {
  if (!start) return "—";
  const seconds = Math.max(
    0,
    Math.floor(
      ((end ? new Date(end) : new Date()).getTime() -
        new Date(start).getTime()) /
        1000,
    ),
  );
  return seconds >= 60
    ? `${Math.floor(seconds / 60)}m ${seconds % 60}s`
    : `${seconds}s`;
}
