import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
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

export type NotificationTone = "success" | "danger" | "warning" | "info";

export interface NotificationInput {
  tone: NotificationTone;
  title: string;
  message?: string;
  actionLabel?: string;
  onAction?: () => void;
  dedupeKey?: string;
  autoDismissMs?: number;
}

export interface Notification extends NotificationInput {
  id: string;
  createdAt: number;
}

export interface NotificationContextValue {
  dismiss: (id: string) => void;
  notify: (input: NotificationInput) => string;
  notifications: Notification[];
}

const NotificationContext = createContext<NotificationContextValue | null>(null);

let nextNotificationId = 1;
const TOAST_MAX_AUTO_DISMISS_MS = 8000;
const TOAST_DEFAULT_AUTO_DISMISS_MS: Record<NotificationTone, number> = {
  success: 4200,
  info: 4200,
  warning: 6500,
  danger: TOAST_MAX_AUTO_DISMISS_MS
};

export function NotificationProvider({ children }: { children: ReactNode }) {
  const [notifications, setNotifications] = useState<Notification[]>([]);
  const timersRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());

  useEffect(
    () => () => {
      for (const timer of timersRef.current.values()) {
        clearTimeout(timer);
      }
      timersRef.current.clear();
    },
    []
  );

  const dismiss = useCallback((id: string) => {
    const timer = timersRef.current.get(id);
    if (timer) {
      clearTimeout(timer);
      timersRef.current.delete(id);
    }
    setNotifications((current) => current.filter((notification) => notification.id !== id));
  }, []);

  const notify = useCallback(
    (input: NotificationInput) => {
      const id = input.dedupeKey ?? `notification_${nextNotificationId++}`;
      const notification: Notification = {
        ...input,
        id,
        createdAt: Date.now()
      };

      const existingTimer = timersRef.current.get(id);
      if (existingTimer) {
        clearTimeout(existingTimer);
        timersRef.current.delete(id);
      }

      setNotifications((current) => {
        const withoutDuplicate = input.dedupeKey
          ? current.filter((item) => item.dedupeKey !== input.dedupeKey)
          : current;
        return [...withoutDuplicate, notification].slice(-6);
      });

      const requestedDismissMs =
        typeof input.autoDismissMs === "number" && input.autoDismissMs > 0
          ? input.autoDismissMs
          : TOAST_DEFAULT_AUTO_DISMISS_MS[input.tone];
      const autoDismissMs = Math.min(requestedDismissMs, TOAST_MAX_AUTO_DISMISS_MS);
      timersRef.current.set(
        id,
        setTimeout(() => dismiss(id), autoDismissMs)
      );

      return id;
    },
    [dismiss]
  );

  const value = useMemo(
    () => ({
      dismiss,
      notify,
      notifications
    }),
    [dismiss, notify, notifications]
  );

  return (
    <NotificationContext.Provider value={value}>
      {children}
      <ToastStack notifications={notifications} onDismiss={dismiss} />
    </NotificationContext.Provider>
  );
}

export function useNotifications() {
  const context = useContext(NotificationContext);
  if (!context) {
    throw new Error("useNotifications must be used inside NotificationProvider.");
  }
  return context;
}

export interface ToastStackProps {
  notifications: Notification[];
  onDismiss: (id: string) => void;
}

export function ToastStack({ notifications, onDismiss }: ToastStackProps) {
  if (notifications.length === 0) {
    return null;
  }

  return (
    <div className="layrs-toast-stack" role="status" aria-live="polite" aria-relevant="additions text">
      {notifications.map((notification) => (
        <article className={`layrs-toast layrs-toast--${notification.tone}`} key={notification.id}>
          <div>
            <strong>{notification.title}</strong>
            {notification.message ? <p>{notification.message}</p> : null}
          </div>
          {notification.actionLabel && notification.onAction ? (
            <button type="button" className="layrs-toast__action" onClick={notification.onAction}>
              {notification.actionLabel}
            </button>
          ) : null}
          <button
            type="button"
            className="layrs-toast__dismiss"
            aria-label={`Dismiss ${notification.title}`}
            onClick={() => onDismiss(notification.id)}
          >
            x
          </button>
        </article>
      ))}
    </div>
  );
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
          data-tab-id={tab.id}
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
