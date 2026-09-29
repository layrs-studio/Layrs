import { useEffect, useState, type HTMLAttributes, type ReactNode } from "react";
import { AlertTriangle, CheckCircle2, Info, XCircle } from "lucide-react";

export type Tone = "info" | "success" | "warning" | "danger";

const toneIcons: Record<Tone, ReactNode> = {
  info: <Info aria-hidden size={18} />,
  success: <CheckCircle2 aria-hidden size={18} />,
  warning: <AlertTriangle aria-hidden size={18} />,
  danger: <XCircle aria-hidden size={18} />,
};

/** Durable situation affecting work (offline, revoked access, recovery needed). */
export function Banner({
  tone = "info",
  title,
  children,
  actions,
  className,
}: {
  tone?: Tone;
  title?: ReactNode;
  children?: ReactNode;
  actions?: ReactNode;
  className?: string;
}) {
  // Only dangers interrupt assistive technology; the rest is announced politely.
  const role = tone === "danger" ? "alert" : "status";
  return (
    <div className={["layrs-banner", `layrs-banner--${tone}`, className].filter(Boolean).join(" ")} role={role}>
      <span className="layrs-banner__icon">{toneIcons[tone]}</span>
      <div className="layrs-banner__body">
        {title && <p className="layrs-banner__title">{title}</p>}
        {children && <div className="layrs-banner__text">{children}</div>}
      </div>
      {actions && <div className="layrs-banner__actions">{actions}</div>}
    </div>
  );
}

/** Explains why a view is empty and what can be done next. */
export function EmptyState({
  icon,
  title,
  children,
  actions,
  compact,
}: {
  icon?: ReactNode;
  title: ReactNode;
  children?: ReactNode;
  actions?: ReactNode;
  compact?: boolean;
}) {
  return (
    <div className={["layrs-empty", compact && "layrs-empty--compact"].filter(Boolean).join(" ")}>
      {icon && <div className="layrs-empty__icon">{icon}</div>}
      <p className="layrs-empty__title">{title}</p>
      {children && <div className="layrs-empty__text">{children}</div>}
      {actions && <div className="layrs-empty__actions">{actions}</div>}
    </div>
  );
}

export function Spinner({ size = 16, label }: { size?: number; label?: string }) {
  return (
    <span className="layrs-spinner" style={{ width: size, height: size }} role={label ? "status" : undefined} aria-hidden={label ? undefined : true}>
      {label && <span className="layrs-visually-hidden">{label}</span>}
    </span>
  );
}

export function Skeleton({ width, height = 14, className }: { width?: number | string; height?: number | string; className?: string }) {
  return <span aria-hidden className={["layrs-skeleton", className].filter(Boolean).join(" ")} style={{ width, height }} />;
}

/**
 * Becomes true only after `delay` ms of `active`, so fast responses never
 * flash a skeleton.
 */
export function useDelayedFlag(active: boolean, delay = 180) {
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!active) {
      setVisible(false);
      return;
    }
    const timer = window.setTimeout(() => setVisible(true), delay);
    return () => window.clearTimeout(timer);
  }, [active, delay]);
  return visible;
}

/** Loading region: keeps the page structure and announces progress once. */
export function Loading({ label = "Chargement…", lines = 3 }: { label?: string; lines?: number }) {
  const visible = useDelayedFlag(true);
  return (
    <div className="layrs-loading" role="status" aria-live="polite">
      <span className="layrs-visually-hidden">{label}</span>
      {visible && Array.from({ length: lines }, (_, index) => <Skeleton key={index} width={`${90 - index * 15}%`} />)}
    </div>
  );
}

export function Badge({ tone = "info", children, className }: { tone?: Tone | "accent"; children: ReactNode; className?: string }) {
  return <span className={["layrs-badge", `layrs-badge--${tone}`, className].filter(Boolean).join(" ")}>{children}</span>;
}

/** Coloured dot that always comes with a text label. */
export function StatusDot({ tone = "info", children }: { tone?: Tone | "accent"; children: ReactNode }) {
  return (
    <span className={`layrs-status layrs-status--${tone}`}>
      <span className="layrs-status__dot" aria-hidden />
      {children}
    </span>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="layrs-kbd">{children}</kbd>;
}

/** Initials only: Layrs never invents a portrait. */
export function Avatar({ name, size = 32 }: { name: string; size?: number }) {
  const initials =
    name
      .split(/[\s._@-]+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((part) => part[0]?.toUpperCase())
      .join("") || "?";
  return (
    <span className="layrs-avatar" style={{ width: size, height: size, fontSize: Math.round(size * 0.4) }} aria-hidden>
      {initials}
    </span>
  );
}

export function Card({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div className={["layrs-card", className].filter(Boolean).join(" ")} {...props} />;
}

/** Monospace identifier with an explicit, named copy action. */
export function TechnicalId({ value, label = "Copier l’identifiant" }: { value: string; label?: string }) {
  const [copied, setCopied] = useState<"idle" | "done" | "failed">("idle");
  useEffect(() => {
    if (copied === "idle") return;
    const timer = window.setTimeout(() => setCopied("idle"), 2000);
    return () => window.clearTimeout(timer);
  }, [copied]);
  return (
    <span className="layrs-technical-id">
      <code title={value}>{value}</code>
      <button
        type="button"
        className="layrs-technical-id__copy"
        onClick={() =>
          navigator.clipboard?.writeText(value).then(
            () => setCopied("done"),
            () => setCopied("failed"),
          )
        }
      >
        {copied === "done" ? "Copié" : copied === "failed" ? "Copie impossible" : label}
      </button>
    </span>
  );
}
