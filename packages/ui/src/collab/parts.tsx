import { useEffect, useState, type ReactNode } from "react";
import { Check, Copy, Lock } from "lucide-react";
import { Button } from "../components/Button";
import { Banner, Skeleton, TechnicalId, useDelayedFlag } from "../components/Feedback";
import { formatDateTime, formatRelativeTime } from "../foundation/format";
import { explainCollabError, type CollabErrorInfo } from "./model";

/** Error that says what failed, what is kept and what to do; the code stays in details. */
export function CollabError({
  error,
  title,
  kept,
  onRetry,
  retryLabel = "Réessayer",
}: {
  error: unknown;
  title?: string;
  kept?: string;
  onRetry?: () => void;
  retryLabel?: string;
}) {
  const info: CollabErrorInfo = explainCollabError(error);
  return (
    <Banner
      tone={info.forbidden ? "warning" : "danger"}
      title={title ?? (info.forbidden ? "Accès refusé" : "L’opération n’a pas abouti")}
      actions={onRetry ? <Button size="sm" onClick={onRetry}>{retryLabel}</Button> : undefined}
    >
      <p>{info.message}</p>
      {kept && <p>{kept}</p>}
      {info.code && (
        <details className="collab-details">
          <summary>Détails techniques</summary>
          <TechnicalId value={info.code} label="Copier le code" />
        </details>
      )}
    </Banner>
  );
}

/** Skeleton rows shown only after a short delay, so fast answers never flash. */
export function DelayedRows({ active, rows = 4, label }: { active: boolean; rows?: number; label: string }) {
  const visible = useDelayedFlag(active);
  if (!active) return null;
  return (
    <div className="collab-skeleton" role="status" aria-live="polite">
      <span className="layrs-visually-hidden">{label}</span>
      {visible &&
        Array.from({ length: rows }, (_, index) => (
          <div key={index} className="collab-skeleton__row">
            <Skeleton width={28} height={28} />
            <div className="collab-skeleton__text">
              <Skeleton width={`${70 - index * 8}%`} />
              <Skeleton width={`${40 - index * 4}%`} height={10} />
            </div>
          </div>
        ))}
    </div>
  );
}

export function CopyButton({ value, label, done = "Copié" }: { value: string; label: string; done?: string }) {
  const [state, setState] = useState<"idle" | "done" | "failed">("idle");
  useEffect(() => {
    if (state === "idle") return;
    const timer = window.setTimeout(() => setState("idle"), 2000);
    return () => window.clearTimeout(timer);
  }, [state]);
  return (
    <Button
      size="sm"
      icon={state === "done" ? <Check aria-hidden size={14} /> : <Copy aria-hidden size={14} />}
      onClick={() =>
        navigator.clipboard?.writeText(value).then(
          () => setState("done"),
          () => setState("failed"),
        ) ?? setState("failed")
      }
    >
      <span aria-live="polite">{state === "done" ? done : state === "failed" ? "Copie impossible : sélectionnez le texte" : label}</span>
    </Button>
  );
}

export function When({ value }: { value: string | null | undefined }) {
  if (!value) return <span>date inconnue</span>;
  return (
    <time dateTime={value} title={formatDateTime(value)}>
      {formatRelativeTime(value)}
    </time>
  );
}

export function LockedContent({ children }: { children?: ReactNode }) {
  return (
    <span className="collab-locked">
      <Lock aria-hidden size={14} />
      {children ?? "Contenu non accessible"}
    </span>
  );
}

/** Collapsible technical references, never the primary information. */
export function TechnicalDetails({ items, summary = "Détails techniques" }: { items: { label: string; value: string | null | undefined; copy?: string }[]; summary?: string }) {
  const shown = items.filter((item): item is { label: string; value: string; copy?: string } => Boolean(item.value));
  if (!shown.length) return null;
  return (
    <details className="collab-details">
      <summary>{summary}</summary>
      <dl className="collab-facts">
        {shown.map((item) => (
          <div key={item.label}>
            <dt>{item.label}</dt>
            <dd>
              <TechnicalId value={item.value} label={item.copy ?? "Copier"} />
            </dd>
          </div>
        ))}
      </dl>
    </details>
  );
}
