import { useEffect, useState, type ReactNode } from "react";
import { Button } from "../components/Button";
import { Banner } from "../components/Feedback";
import { Dialog } from "../components/Dialog";
import { useToast } from "../components/Toast";
import { describeError } from "./errors";
import type { PolicyPreview, PolicyReceipt, PolicyTransport } from "./types";

export type PreparedPolicyChange = {
  title: string;
  /** Person/object and scope named in full. */
  subject: ReactNode;
  /** Before → after and side effects, in plain words. */
  effects: ReactNode[];
  /** Server-bound preview: expected PolicySet and authorization epoch. */
  preview: PolicyPreview;
  confirmLabel: string;
  successTitle: string;
  danger?: boolean;
};

type NativeIntent = PolicyPreview & {
  origin?: string;
  principalId?: string;
  worktreeId?: string;
  request?: { clientId?: string; idempotencyKey?: string; requestDigest?: string };
};
const uncertainInMemory = new Set<string>();

/** Only non-secret identity fields; no policy document, preview, or token is stored. */
export function policyUncertaintyKey(preview: PolicyPreview): string | null {
  const native = preview as NativeIntent;
  const request = native.request;
  if (!native.origin || !native.principalId || !native.worktreeId || !request?.clientId || !request.idempotencyKey || !request.requestDigest)
    return null;
  return `layrs.policy-uncertain.v1:${JSON.stringify([native.origin, native.principalId, native.spaceId, native.worktreeId, request.clientId, request.idempotencyKey, request.requestDigest])}`;
}

export function policyWasUncertain(preview: PolicyPreview): boolean {
  const key = policyUncertaintyKey(preview);
  if (!key) return false;
  if (uncertainInMemory.has(key)) return true;
  try { return window.localStorage.getItem(key) === "1"; } catch { return false; }
}

export function rememberPolicyUncertain(preview: PolicyPreview): void {
  const key = policyUncertaintyKey(preview);
  if (!key) return;
  uncertainInMemory.add(key);
  try { window.localStorage.setItem(key, "1"); } catch { /* In-memory fallback for restricted storage. */ }
}

export function clearPolicyUncertain(preview: PolicyPreview): void {
  const key = policyUncertaintyKey(preview);
  if (!key) return;
  uncertainInMemory.delete(key);
  try { window.localStorage.removeItem(key); } catch { /* In-memory fallback for restricted storage. */ }
}

/** Keep an earlier uncertain apply uncertain even if a later retry is rejected before dispatch. */
export function policyApplyFailure(cause: unknown, previouslyUncertain: boolean) {
  if (previouslyUncertain)
    return {
      uncertain: true,
      message: `Une tentative précédente reste sans réponse : elle a peut-être été appliquée. ${describeError(cause, "Cette nouvelle tentative n’a pas confirmé le résultat.")}`,
    };
  const status = (cause as { status?: unknown })?.status;
  const beforeDispatch = (cause as { dispatched?: unknown })?.dispatched === false;
  if (beforeDispatch || (typeof status === "number" && status >= 400 && status < 500))
    return { uncertain: false, message: describeError(cause, "La modification n’a pas été appliquée.") };
  return {
    uncertain: true,
    message: "La réponse du serveur n’est pas arrivée. Le changement a peut-être été appliqué : réessayez ce même aperçu pour connaître le résultat, il ne sera jamais appliqué deux fois.",
  };
}

/**
 * Confirms and applies one prepared policy change. The server compares the
 * expected PolicySet and epoch; a concurrent change is refused. A lost answer
 * is retried with the exact same preview, which is never applied twice.
 */
export function PolicyChangeDialog({
  value,
  policy,
  onClose,
  onApplied,
  onReceipt,
}: {
  value: PreparedPolicyChange | null;
  policy: PolicyTransport;
  onClose: () => void;
  /** Re-reads rules and rights; the change is only reported once re-read. */
  onApplied: () => Promise<void>;
  /** An authoritative applied or rejected receipt releases a recovered pending intent. */
  onReceipt?: (receipt: PolicyReceipt) => void;
}) {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [uncertain, setUncertain] = useState(false);
  useEffect(() => {
    setError("");
    setUncertain(value ? policyWasUncertain(value.preview) : false);
  }, [value]);

  async function apply() {
    if (!value) return;
    const previouslyUncertain = uncertain || policyWasUncertain(value.preview);
    setBusy(true);
    setError("");
    // A process can stop while the request is in flight, before a network
    // failure reaches catch. Keep the exact intent uncertain until a receipt
    // or a proven refusal before dispatch clears it.
    rememberPolicyUncertain(value.preview);
    try {
      const { receipt } = await policy<{ receipt: PolicyReceipt }>({ action: "applyPolicy", preview: value.preview });
      setUncertain(false);
      clearPolicyUncertain(value.preview);
      onReceipt?.(receipt);
      if (receipt.status !== "applied") {
        const codes = receipt.rejections?.map((item) => item.code).join(", ");
        setError(`Le serveur a refusé la modification${codes ? ` (${codes})` : ""}. Rien n’a changé. Fermez, rechargez les règles et préparez un nouvel aperçu.`);
        return;
      }
      try {
        await onApplied();
        toast.show({ tone: "success", title: value.successTitle, description: "Les droits ont été relus sur le serveur." });
      } catch {
        toast.show({ tone: "warning", title: value.successTitle, description: "Appliqué, mais la relecture a échoué : vos propres droits ont peut-être changé. Rechargez la page." });
      }
      onClose();
    } catch (cause) {
      const failure = policyApplyFailure(cause, previouslyUncertain);
      setUncertain(failure.uncertain);
      if (!failure.uncertain) clearPolicyUncertain(value.preview);
      setError(failure.message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      open={value !== null}
      onClose={() => !busy && onClose()}
      dismissible={!busy}
      size="lg"
      title={value?.title ?? ""}
      footer={
        value && (
          <>
            <Button onClick={onClose} disabled={busy}>
              {uncertain ? "Fermer" : "Annuler"}
            </Button>
            <Button variant={value.danger ? "danger" : "primary"} busy={busy} busyLabel="Application…" onClick={() => void apply()}>
              {uncertain ? "Réessayer ce même aperçu" : value.confirmLabel}
            </Button>
          </>
        )
      }
    >
      {value && (
        <div className="team-confirm">
          <p className="team-confirm__subject">{value.subject}</p>
          <ul className="team-confirm__effects">
            {value.effects.map((effect, index) => (
              <li key={index}>{effect}</li>
            ))}
          </ul>
          {uncertain && !error && <Banner tone="warning">Le résultat de cette demande exacte reste à confirmer. Réessayez ce même aperçu pour obtenir son reçu, sans créer une nouvelle modification.</Banner>}
          <details className="team-details">
            <summary>Références techniques de l’aperçu</summary>
            <dl className="settings-facts">
              <dt>Politique actuelle</dt>
              <dd>
                <code>{value.preview.expectedPolicySetId}</code>
              </dd>
              <dt>Politique proposée</dt>
              <dd>
                <code>{value.preview.proposedPolicySetId}</code>
              </dd>
              <dt>Révision des droits attendue</dt>
              <dd>{value.preview.expectedAuthorizationEpoch}</dd>
              {value.preview.inlineRules?.replacement && (
                <>
                  <dt>Politique remplacée</dt>
                  <dd>
                    <code>{value.preview.inlineRules.replacement.policyId}</code>
                  </dd>
                </>
              )}
            </dl>
            {value.preview.inlineRules && <pre className="team-json">{JSON.stringify(value.preview.inlineRules.rules, null, 2)}</pre>}
          </details>
          {error && <Banner tone={uncertain ? "warning" : "danger"}>{error}</Banner>}
        </div>
      )}
    </Dialog>
  );
}
