import { useMemo, useState } from "react";
import type { ReactNode } from "react";
import type {
  Artifact,
  Gate,
  GateStatus,
  Policy,
  Proof,
  WeaveEvent
} from "@layrs/client-sdk";

export interface SidebarItem {
  id: string;
  label: string;
  eyebrow?: string;
  isActive?: boolean;
  meta?: string;
}

export interface AppShellProps {
  productName: string;
  workspaceName: string;
  sidebar: ReactNode;
  toolbar?: ReactNode;
  children: ReactNode;
}

export function AppShell({ productName, workspaceName, sidebar, toolbar, children }: AppShellProps) {
  return (
    <div className="layrs-shell">
      <aside className="layrs-shell__sidebar">
        <div className="layrs-brand">
          <span className="layrs-brand__mark">L</span>
          <div>
            <strong>{productName}</strong>
            <span>{workspaceName}</span>
          </div>
        </div>
        {sidebar}
      </aside>
      <div className="layrs-shell__main">
        {toolbar ? <header className="layrs-toolbar">{toolbar}</header> : null}
        <main className="layrs-workspace">{children}</main>
      </div>
    </div>
  );
}

export interface SidebarProps {
  items: SidebarItem[];
  footer?: ReactNode;
}

export function Sidebar({ items, footer }: SidebarProps) {
  return (
    <nav className="layrs-sidebar" aria-label="Studio navigation">
      <div className="layrs-sidebar__items">
        {items.map((item) => (
          <a
            className={item.isActive ? "layrs-sidebar__item is-active" : "layrs-sidebar__item"}
            href={`#${item.id}`}
            key={item.id}
          >
            <span>
              {item.eyebrow ? <small>{item.eyebrow}</small> : null}
              <strong>{item.label}</strong>
            </span>
            {item.meta ? <em>{item.meta}</em> : null}
          </a>
        ))}
      </div>
      {footer ? <div className="layrs-sidebar__footer">{footer}</div> : null}
    </nav>
  );
}

export interface StatusPillProps {
  status: GateStatus | Proof["status"];
  label?: string;
}

export interface ActionGroupProps {
  align?: "start" | "end" | "between";
  children: ReactNode;
}

export function ActionGroup({ align = "end", children }: ActionGroupProps) {
  return <div className={`layrs-action-group layrs-action-group--${align}`}>{children}</div>;
}

export interface TabItem {
  id: string;
  label: string;
  count?: number;
  disabled?: boolean;
  note?: string;
}

export interface TabsProps {
  activeId: string;
  ariaLabel: string;
  onChange: (id: string) => void;
  tabs: TabItem[];
}

export function Tabs({ activeId, ariaLabel, onChange, tabs }: TabsProps) {
  return (
    <div className="layrs-tabs" role="tablist" aria-label={ariaLabel}>
      {tabs.map((tab) => (
        <button
          aria-selected={activeId === tab.id}
          className={activeId === tab.id ? "is-active" : undefined}
          disabled={tab.disabled}
          key={tab.id}
          onClick={() => onChange(tab.id)}
          role="tab"
          title={tab.note}
          type="button"
        >
          <span>{tab.label}</span>
          {typeof tab.count === "number" ? <em>{tab.count}</em> : null}
          {tab.disabled && tab.note ? <small>{tab.note}</small> : null}
        </button>
      ))}
    </div>
  );
}

export interface DangerZoneProps {
  children: ReactNode;
  description: string;
  title: string;
}

export function DangerZone({ children, description, title }: DangerZoneProps) {
  return (
    <section className="layrs-danger-zone">
      <div>
        <strong>{title}</strong>
        <p>{description}</p>
      </div>
      <div className="layrs-danger-zone__actions">{children}</div>
    </section>
  );
}

export interface ConfirmModalProps {
  cancelLabel?: string;
  confirmLabel: string;
  confirmationLabel?: string;
  confirmationValue?: string;
  danger?: boolean;
  description: ReactNode;
  disabled?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
  onConfirmationValueChange?: (value: string) => void;
  open: boolean;
  title: string;
}

export function ConfirmModal({
  cancelLabel = "Cancel",
  confirmLabel,
  confirmationLabel,
  confirmationValue,
  danger = false,
  description,
  disabled,
  onCancel,
  onConfirm,
  onConfirmationValueChange,
  open,
  title
}: ConfirmModalProps) {
  if (!open) {
    return null;
  }

  return (
    <div className="layrs-confirm-backdrop" role="presentation">
      <section className="layrs-confirm" role="dialog" aria-modal="true" aria-labelledby="layrs-confirm-title">
        <div className="layrs-confirm__header">
          <span>{danger ? "Destructive action" : "Confirm action"}</span>
          <h2 id="layrs-confirm-title">{title}</h2>
        </div>
        <div className="layrs-confirm__body">{description}</div>
        {confirmationLabel ? (
          <label className="layrs-confirm__field">
            <span>{confirmationLabel}</span>
            <input
              autoFocus
              value={confirmationValue ?? ""}
              onChange={(event) => onConfirmationValueChange?.(event.currentTarget.value)}
            />
          </label>
        ) : null}
        <div className="layrs-confirm__actions">
          <button type="button" className="layrs-button layrs-button--secondary" onClick={onCancel}>
            {cancelLabel}
          </button>
          <button
            type="button"
            className={danger ? "layrs-button layrs-button--danger" : "layrs-button layrs-button--primary"}
            disabled={disabled}
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </section>
    </div>
  );
}

const statusLabels: Record<string, string> = {
  passing: "Passing",
  blocked: "Blocked",
  "needs-proof": "Needs proof",
  pending: "Pending",
  accepted: "Accepted",
  missing: "Missing",
  stale: "Stale",
  reviewing: "Reviewing"
};

export function StatusPill({ status, label }: StatusPillProps) {
  return (
    <span className={`layrs-pill layrs-pill--${status}`}>
      <span className="layrs-pill__dot" />
      {label ?? statusLabels[status] ?? status}
    </span>
  );
}

export interface VirtualWeaveListProps {
  events: WeaveEvent[];
  height?: number;
  rowHeight?: number;
}

export function VirtualWeaveList({ events, height = 420, rowHeight = 112 }: VirtualWeaveListProps) {
  const [scrollTop, setScrollTop] = useState(0);
  const overscan = 5;
  const visibleCount = Math.ceil(height / rowHeight);
  const startIndex = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const endIndex = Math.min(events.length, startIndex + visibleCount + overscan * 2);

  const visibleEvents = useMemo(
    () => events.slice(startIndex, endIndex),
    [endIndex, events, startIndex]
  );

  return (
    <div
      className="layrs-weave-list"
      onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
      style={{ height }}
    >
      <div className="layrs-weave-list__spacer" style={{ height: events.length * rowHeight }}>
        <div
          className="layrs-weave-list__window"
          style={{ transform: `translateY(${startIndex * rowHeight}px)` }}
        >
          {visibleEvents.map((event) => (
            <article className="layrs-weave-row" key={event.id} style={{ height: rowHeight }}>
              <div className="layrs-weave-row__rail">
                <span>{event.kind}</span>
                <time>{formatTime(event.at)}</time>
              </div>
              <div className="layrs-weave-row__body">
                <div className="layrs-weave-row__title">
                  <strong>{event.title}</strong>
                  <span>{event.actor}</span>
                </div>
                <p>{event.summary}</p>
                {event.diffStats ? (
                  <dl className="layrs-diff-stats" aria-label="Diff stats">
                    <div>
                      <dt>Files</dt>
                      <dd>{event.diffStats.files}</dd>
                    </div>
                    <div>
                      <dt>Added</dt>
                      <dd>+{event.diffStats.additions}</dd>
                    </div>
                    <div>
                      <dt>Removed</dt>
                      <dd>-{event.diffStats.removals}</dd>
                    </div>
                  </dl>
                ) : null}
              </div>
            </article>
          ))}
        </div>
      </div>
    </div>
  );
}

export interface ArtifactCardProps {
  artifact: Artifact;
  proofs?: Proof[];
}

export function ArtifactCard({ artifact, proofs = [] }: ArtifactCardProps) {
  return (
    <article className="layrs-artifact-card">
      <div className="layrs-artifact-card__top">
        <span>{artifact.type}</span>
        <time>{formatDate(artifact.updatedAt)}</time>
      </div>
      <strong>{artifact.name}</strong>
      <p>{artifact.summary}</p>
      <footer>
        <code>{artifact.location}</code>
        <span>{artifact.sizeLabel}</span>
      </footer>
      {proofs.length > 0 ? (
        <div className="layrs-artifact-card__proofs">
          {proofs.map((proof) => (
            <StatusPill key={proof.id} status={proof.status} label={proof.title} />
          ))}
        </div>
      ) : null}
    </article>
  );
}

export interface ProofPanelProps {
  gates: Gate[];
  proofs: Proof[];
}

export function ProofPanel({ gates, proofs }: ProofPanelProps) {
  return (
    <section className="layrs-proof-panel" aria-labelledby="proof-panel-title">
      <div className="layrs-section-heading">
        <span>Proofs and Gates</span>
        <h2 id="proof-panel-title">Gate readiness</h2>
      </div>
      <div className="layrs-proof-panel__gates">
        {gates.map((gate) => {
          const gateProofs = proofs.filter((proof) => proof.gateId === gate.id);
          return (
            <article className="layrs-gate-row" key={gate.id}>
              <div>
                <strong>{gate.name}</strong>
                <p>{gate.summary}</p>
              </div>
              <StatusPill status={gate.status} />
              <div className="layrs-gate-row__proofs">
                {gateProofs.map((proof) => (
                  <span key={proof.id}>
                    {proof.kind}: {proof.status}
                  </span>
                ))}
              </div>
            </article>
          );
        })}
      </div>
    </section>
  );
}

export interface PolicyMatrixProps {
  policies: Policy[];
}

export function PolicyMatrix({ policies }: PolicyMatrixProps) {
  return (
    <section className="layrs-policy-matrix" aria-labelledby="policy-matrix-title">
      <div className="layrs-section-heading">
        <span>Policy editor</span>
        <h2 id="policy-matrix-title">Rules matrix</h2>
      </div>
      <div className="layrs-policy-matrix__table" role="table" aria-label="Policy rules">
        <div className="layrs-policy-matrix__head" role="row">
          <span role="columnheader">Policy</span>
          <span role="columnheader">Scope</span>
          <span role="columnheader">Applies to</span>
          <span role="columnheader">Effect</span>
          <span role="columnheader">Rules</span>
        </div>
        {policies.map((policy) => (
          <div className="layrs-policy-matrix__row" role="row" key={policy.id}>
            <strong role="cell">{policy.name}</strong>
            <span role="cell">{policy.scope}</span>
            <span role="cell">{policy.appliesTo}</span>
            <span role="cell">
              <StatusPill status={policy.effect === "block" ? "blocked" : policy.effect === "allow" ? "passing" : "needs-proof"} label={policy.effect} />
            </span>
            <span role="cell">{policy.rules.map((rule) => `${rule.action}: ${rule.effect}`).join(", ")}</span>
          </div>
        ))}
      </div>
    </section>
  );
}

function formatDate(value: string) {
  return new Intl.DateTimeFormat("en", { month: "short", day: "2-digit" }).format(new Date(value));
}

function formatTime(value: string) {
  return new Intl.DateTimeFormat("en", { hour: "2-digit", minute: "2-digit" }).format(new Date(value));
}
