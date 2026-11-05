import { useState, type ReactNode } from "react";
import { Button } from "../components/Button";
import { Banner } from "../components/Feedback";
import { Dialog } from "../components/Dialog";
import { describeError } from "./errors";

export type Confirmation = {
  title: string;
  /** Names the object and its scope, e.g. "Bob Martin · projet Atelier". */
  subject: ReactNode;
  /** What will happen, including effects on others. */
  effects: ReactNode[];
  confirmLabel: string;
  busyLabel?: string;
  danger?: boolean;
  /** Runs on confirmation; throw to keep the dialog open with the error. */
  run: () => Promise<void>;
};

/**
 * Sensitive action confirmation: object, scope and effects are spelled out;
 * the dialog stays open on failure so the reason is read in context.
 */
export function ConfirmDialog({ value, onClose }: { value: Confirmation | null; onClose: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const close = () => {
    if (busy) return;
    setError("");
    onClose();
  };
  return (
    <Dialog
      open={value !== null}
      onClose={close}
      dismissible={!busy}
      title={value?.title ?? ""}
      footer={
        value && (
          <>
            <Button onClick={close} disabled={busy}>
              Annuler
            </Button>
            <Button
              variant={value.danger ? "danger" : "primary"}
              busy={busy}
              busyLabel={value.busyLabel ?? "Envoi…"}
              onClick={async () => {
                setBusy(true);
                setError("");
                try {
                  await value.run();
                  setBusy(false);
                  onClose();
                } catch (cause) {
                  setBusy(false);
                  setError(describeError(cause, "Rien n’a été modifié."));
                }
              }}
            >
              {value.confirmLabel}
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
          {error && <Banner tone="danger">{error}</Banner>}
        </div>
      )}
    </Dialog>
  );
}
