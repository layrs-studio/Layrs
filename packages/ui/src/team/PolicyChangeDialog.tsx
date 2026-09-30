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
}: {
  value: PreparedPolicyChange | null;
  policy: PolicyTransport;
  onClose: () => void;
  /** Re-reads rules and rights; the change is only reported once re-read. */
  onApplied: () => Promise<void>;
}) {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [uncertain, setUncertain] = useState(false);
  useEffect(() => {
    setError("");
    setUncertain(false);
  }, [value]);

  async function apply() {
    if (!value) return;
    setBusy(true);
    setError("");
    try {
      const { receipt } = await policy<{ receipt: PolicyReceipt }>({ action: "applyPolicy", preview: value.preview });
      setUncertain(false);
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
      const status = (cause as { status?: unknown })?.status;
      if (typeof status === "number" && status >= 400 && status < 500) setError(describeError(cause, "La modification n’a pas été appliquée."));
      else {
        setUncertain(true);
        setError(
          "La réponse du serveur n’est pas arrivée. Le changement a peut-être été appliqué : réessayez ce même aperçu pour connaître le résultat, il ne sera jamais appliqué deux fois.",
        );
      }
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
