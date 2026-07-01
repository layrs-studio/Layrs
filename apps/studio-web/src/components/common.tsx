import type { FormEvent, ReactNode } from "react";
import type { Workspace } from "@layrs/client-sdk";

export function WorkspaceSwitcher({
  activeWorkspaceId,
  onChange,
  workspaces
}: {
  activeWorkspaceId: string;
  onChange: (workspaceId: string) => void;
  workspaces: Workspace[];
}) {
  return (
    <label className="studio-workspace-switcher">
      <span>Workspace</span>
      <select onChange={(event) => onChange(event.currentTarget.value)} value={activeWorkspaceId}>
        {workspaces.map((workspace) => (
          <option key={workspace.id} value={workspace.id}>
            {workspace.name}
          </option>
        ))}
      </select>
    </label>
  );
}

export function PanelTitle({ eyebrow, title }: { eyebrow: string; title: string }) {
  return (
    <div className="layrs-section-heading">
      <span>{eyebrow}</span>
      <h2>{title}</h2>
    </div>
  );
}

export function Metric({ label, value }: { label: string; value: number }) {
  return (
    <div className="studio-metric">
      <strong>{value}</strong>
      <span>{label}</span>
    </div>
  );
}

export function TextField({
  label,
  name,
  pattern,
  required,
  type = "text"
}: {
  label: string;
  name: string;
  pattern?: string;
  required?: boolean;
  type?: string;
}) {
  return (
    <label className="studio-field">
      <span>{label}</span>
      <input autoComplete={name} name={name} pattern={pattern} required={required} type={type} />
    </label>
  );
}

export function EmptyState({ detail, title }: { detail: string; title: string }) {
  return (
    <div className="studio-empty-state">
      <strong>{title}</strong>
      <p>{detail}</p>
    </div>
  );
}

export function InlineAlert({ children, tone }: { children: ReactNode; tone: "danger" | "success" | "warning" }) {
  return <div className={`studio-alert studio-alert--${tone}`}>{children}</div>;
}

export function SystemScreen({
  actionLabel,
  detail,
  onAction,
  title
}: {
  actionLabel?: string;
  detail: string;
  onAction?: () => void;
  title: string;
}) {
  return (
    <div className="studio-system-screen">
      <span className="studio-auth-mark">L</span>
      <h1>{title}</h1>
      <p>{detail}</p>
      {actionLabel && onAction ? (
        <button className="studio-primary-button" onClick={onAction} type="button">
          {actionLabel}
        </button>
      ) : null}
    </div>
  );
}

export function submitForm(handler: (form: FormData) => Promise<void>) {
  return async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    await handler(new FormData(event.currentTarget));
    event.currentTarget.reset();
  };
}

export function formatDate(value: string) {
  return new Intl.DateTimeFormat("en", { month: "short", day: "2-digit" }).format(new Date(value));
}

export function formatTime(value: string) {
  return new Intl.DateTimeFormat("en", { hour: "2-digit", minute: "2-digit" }).format(new Date(value));
}
