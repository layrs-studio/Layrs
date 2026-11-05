import { useMemo, useState, type FormEvent } from "react";
import { LogOut } from "lucide-react";
import { Avatar, Banner, Button, Card, PageHeader, PasswordField, TechnicalId, TokensPanel, useToast } from "@layrs/ui";
import { administrationTransport } from "../app/transports";
import { ApiError, useSession } from "../app/session";

const kindLabels: Record<string, string> = {
  bootstrap_admin: "Administrateur de l’instance",
  account: "Compte personnel",
  service: "Compte de service",
};

/** P20: who am I, where, and how to leave or change my password. */
export function AccountPage() {
  const { principal, api, signOut, forget, sessionTokenId } = useSession();
  const identity = useMemo(() => administrationTransport(api, ""), [api]);
  const toast = useToast();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const invalid =
    !current || !next
      ? "Remplissez les deux mots de passe."
      : next.length < 12
        ? "Le nouveau mot de passe doit contenir au moins 12 caractères."
        : next !== confirm
          ? "La confirmation ne correspond pas."
          : undefined;

  async function changePassword(event: FormEvent) {
    event.preventDefault();
    if (invalid || busy) return;
    setBusy(true);
    setError("");
    try {
      await api("/v3/auth/password", "POST", { current_password: current, new_password: next }, { unauthorizedIsAnswer: true });
      toast.show({ tone: "success", title: "Mot de passe modifié", description: "Toutes vos sessions ont été fermées. Reconnectez-vous." });
      forget();
    } catch (cause) {
      setError(
        cause instanceof ApiError && cause.status === 401
          ? "Mot de passe actuel incorrect, ou session expirée. Votre mot de passe n’a pas changé."
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
    } finally {
      setBusy(false);
      setCurrent("");
    }
  }

  if (!principal) return null;
  return (
    <div className="layrs-page layrs-page--narrow">
      <PageHeader title="Compte" description="Votre identité sur cette instance et la sécurité de votre accès." />
      <Card className="settings-card">
        <div className="account-identity">
          <Avatar name={principal.displayName || principal.principalId} size={48} />
          <div>
            <p className="account-identity__name">{principal.displayName || "Nom non renseigné"}</p>
            <p className="account-identity__meta">
              {kindLabels[principal.kind] ?? principal.kind} · {location.host}
            </p>
          </div>
        </div>
        <dl className="settings-facts">
          <dt>Identifiant technique</dt>
          <dd>
            <TechnicalId value={principal.principalId} />
          </dd>
        </dl>
        <div>
          <Button icon={<LogOut aria-hidden size={16} />} onClick={() => void signOut()}>
            Se déconnecter
          </Button>
        </div>
      </Card>
      <section className="settings-section" aria-labelledby="sessions-title">
        <h2 id="sessions-title">Sessions et appareils</h2>
        <p className="settings-section__hint">Connexions actives de votre compte. Révoquer une session coupe immédiatement l’accès de l’appareil ou de l’outil concerné.</p>
        <TokensPanel
          scope={`${location.origin}:${principal.principalId}`}
          administration={identity}
          currentTokenId={sessionTokenId}
          onCurrentRevoked={() => {
            toast.show({ tone: "success", title: "Session fermée", description: "Reconnectez-vous pour continuer." });
            forget();
          }}
        />
      </section>
      <section className="settings-section" aria-labelledby="password-title">
        <h2 id="password-title">Changer le mot de passe</h2>
        <p className="settings-section__hint">Toutes vos sessions, y compris celle-ci et vos appareils connectés, seront fermées.</p>
        <Card>
          <form className="settings-card" onSubmit={(event) => void changePassword(event)}>
            {error && <Banner tone="danger">{error}</Banner>}
            <PasswordField label="Mot de passe actuel" autoComplete="current-password" value={current} onChange={(event) => setCurrent(event.target.value)} />
            <PasswordField
              label="Nouveau mot de passe"
              autoComplete="new-password"
              hint="Au moins 12 caractères."
              value={next}
              onChange={(event) => setNext(event.target.value)}
            />
            <PasswordField label="Confirmer le nouveau mot de passe" autoComplete="new-password" value={confirm} onChange={(event) => setConfirm(event.target.value)} />
            <div>
              <Button type="submit" variant="primary" busy={busy} busyLabel="Modification…" disabledReason={invalid}>
                Changer le mot de passe
              </Button>
            </div>
          </form>
        </Card>
      </section>
    </div>
  );
}
