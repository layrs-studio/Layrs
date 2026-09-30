import { useEffect, useRef, useState } from "react";
import { Copy, KeyRound } from "lucide-react";
import { Button } from "../components/Button";
import { Banner } from "../components/Feedback";
import { Dialog } from "../components/Dialog";

export type RevealedSecret = {
  title: string;
  /** Readable owner of the secret, e.g. "Automatisation CI · jeton « runner »". */
  owner: string;
  /** What to do with it and what happens to older secrets. */
  guidance: string;
  value: string;
};

const TIMEOUT_SECONDS = 60;

/**
 * One-time display of a secret returned at creation or rotation. It is kept
 * only in this component's memory, masked by default, and forgotten after
 * 60 seconds or on acknowledgement. It is never shown again.
 */
export function SecretReveal({ secret, onDone }: { secret: RevealedSecret | null; onDone: () => void }) {
  const [shown, setShown] = useState(false);
  const [remaining, setRemaining] = useState(TIMEOUT_SECONDS);
  const [copy, setCopy] = useState<"idle" | "done" | "failed">("idle");
  const done = useRef(onDone);
  done.current = onDone;

  useEffect(() => {
    if (!secret) return;
    setShown(false);
    setCopy("idle");
    setRemaining(TIMEOUT_SECONDS);
    const started = Date.now();
    const timer = window.setInterval(() => {
      const left = TIMEOUT_SECONDS - Math.floor((Date.now() - started) / 1000);
      if (left <= 0) {
        window.clearInterval(timer);
        done.current();
      } else setRemaining(left);
    }, 1000);
    return () => window.clearInterval(timer);
  }, [secret]);

  return (
    <Dialog
      open={secret !== null}
      onClose={onDone}
      dismissible={false}
      title={secret?.title ?? ""}
      description={secret?.owner}
      footer={
        <Button variant="primary" onClick={onDone}>
          J’ai enregistré le secret
        </Button>
      }
    >
      {secret && (
        <div className="team-secret" aria-label="Secret créé">
          <Banner tone="warning" title="Affiché une seule fois">
            {secret.guidance} Il sera masqué dans {remaining} s et Layrs ne pourra plus l’afficher.
          </Banner>
          <div className="team-secret__value">
            <KeyRound aria-hidden size={16} />
            <input
              aria-label="Secret"
              type={shown ? "text" : "password"}
              readOnly
              value={secret.value}
              autoComplete="off"
              spellCheck={false}
              onFocus={(event) => event.currentTarget.select()}
            />
          </div>
          <div className="team-actions">
            <Button size="sm" onClick={() => setShown((value) => !value)} aria-pressed={shown}>
              {shown ? "Masquer" : "Afficher"}
            </Button>
            <Button
              size="sm"
              icon={<Copy aria-hidden size={14} />}
              onClick={() =>
                navigator.clipboard?.writeText(secret.value).then(
                  () => setCopy("done"),
                  () => setCopy("failed"),
                )
              }
            >
              {copy === "done" ? "Secret copié" : "Copier le secret"}
            </Button>
          </div>
          {copy === "failed" && <p className="team-muted">Copie impossible : affichez le secret et sélectionnez-le.</p>}
        </div>
      )}
    </Dialog>
  );
}
