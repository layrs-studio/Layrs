import { useCallback, useEffect, useRef, useState } from "react";
import { KeyRound } from "lucide-react";
import { Button } from "../components/Button";
import { Badge, Banner, EmptyState, Loading } from "../components/Feedback";
import { useToast } from "../components/Toast";
import { timeOf, when } from "./time";
import { ConfirmDialog, type Confirmation } from "./ConfirmDialog";
import type { TeamTransport, Token } from "./types";
import { useTask } from "./useTask";

const kinds: Record<string, string> = { session: "Session de connexion", device: "Appareil connecté", service: "Jeton de service" };
const title = (token: Token) => (token.kind === "session" && token.label === "login" ? "Connexion par identifiant" : token.label || kinds[token.kind] || token.kind);

/** P20 · Sessions and tokens of this account; the current session is named when known. */
export function TokensPanel({
  scope,
  administration,
  currentTokenId,
  onCurrentRevoked,
}: {
  scope: string;
  administration: TeamTransport;
  /** Digest of the token this application uses, when known. */
  currentTokenId?: string | null;
  /** The current session was revoked: the application must forget it now. */
  onCurrentRevoked: () => void;
}) {
  const task = useTask(scope);
  const toast = useToast();
  const [tokens, setTokens] = useState<Token[] | null>(null);
  const [confirm, setConfirm] = useState<Confirmation | null>(null);
  const load = useCallback(
    () =>
      task.run("load", async (live) => {
        const value = await administration<{ tokens: Token[] } | Token[]>({ action: "listTokens" });
        if (live()) setTokens(Array.isArray(value) ? value : value.tokens);
      }, "Vos sessions n’ont pas pu être lues."),
    [administration, task],
  );
  const started = useRef("");
  useEffect(() => {
    if (started.current === scope) return;
    started.current = scope;
    void load();
  }, [scope, load]);

  const active = (tokens ?? []).filter((token) => !token.revoked && timeOf(token.expiresAt) > Date.now());
  const inactive = (tokens ?? []).filter((token) => !active.includes(token));
  return (
    <div className="team-panel">
      {task.error && <Banner tone="danger">{task.error}</Banner>}
      {!tokens ? (
        <Loading label="Lecture des sessions…" />
      ) : !tokens.length ? (
        <EmptyState compact icon={<KeyRound size={22} />} title="Aucune session enregistrée" />
      ) : (
        <ul className="team-list" aria-label="Sessions et jetons">
          {[...active, ...inactive].map((token) => {
            const current = token.tokenId === currentTokenId;
            const expired = !token.revoked && timeOf(token.expiresAt) <= Date.now();
            return (
              <li key={token.tokenId} className="team-row">
                <KeyRound aria-hidden size={18} />
                <div className="team-row__main">
                  <p className="team-row__title">
                    {title(token)}
                    {current && <Badge tone="accent">Cette session</Badge>}
                  </p>
                  <p className="team-row__meta">
                    {kinds[token.kind] ?? token.kind} · {token.revoked ? "révoqué" : expired ? `expiré le ${when(token.expiresAt)}` : `expire le ${when(token.expiresAt)}`}
                  </p>
                </div>
                {!token.revoked && !expired && (
                  <div className="team-row__aside">
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() =>
                        setConfirm({
                          title: current ? "Fermer cette session" : "Révoquer la session",
                          subject: `${title(token)}${current ? " · session utilisée ici" : ""}`,
                          effects: current
                            ? ["Vous serez déconnecté immédiatement de ce navigateur.", "Vos autres sessions et appareils restent connectés."]
                            : ["L’appareil ou l’outil qui l’utilise perd l’accès immédiatement.", "Il devra se reconnecter pour continuer."],
                          confirmLabel: current ? "Fermer cette session" : "Révoquer",
                          danger: true,
                          run: async () => {
                            await administration({ action: "revokeToken", tokenId: token.tokenId });
                            if (current) {
                              onCurrentRevoked();
                              return;
                            }
                            toast.show({ tone: "success", title: "Session révoquée" });
                            await load();
                          },
                        })
                      }
                    >
                      Révoquer
                    </Button>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {currentTokenId === undefined && <p className="team-muted team-small">La session utilisée ici n’est pas identifiable dans cette liste.</p>}
      <ConfirmDialog value={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}
