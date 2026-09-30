import { useEffect, useState } from "react";
import { CheckCircle2, ExternalLink, KeyRound, LogOut, RefreshCw } from "lucide-react";
import { Banner, Button, Card, Spinner, TextAreaField, TextField, useToast } from "@layrs/ui";
import { useRemote } from "../app/remote";
import { hostOf, normalizeOrigin } from "../app/serverMemory";
import { NoticeView } from "./parts";

function groupCode(code: string) {
  const compact = code.replace(/[\s-]/g, "");
  if (compact.length < 8 || compact.length % 2) return code;
  return `${compact.slice(0, compact.length / 2)}-${compact.slice(compact.length / 2)}`;
}

function timeOf(timestamp: number) {
  return new Intl.DateTimeFormat("fr-FR", { hour: "2-digit", minute: "2-digit" }).format(timestamp);
}

/**
 * P02: connect Desktop to a server with the device flow. The browser
 * authorizes the device; Desktop follows the same operation. The enterprise
 * certificate lives in an advanced step, never on the daily screens.
 */
export function ConnectFlow({ onDone, doneLabel = "Revenir au projet" }: { onDone?: () => void; doneLabel?: string }) {
  const remote = useRemote();
  const toast = useToast();
  const { session, login, origin, fixed } = remote;
  const [address, setAddress] = useState(origin);
  const [pem, setPem] = useState(remote.authority);
  const [editing, setEditing] = useState(!origin || session.state !== "connected");

  useEffect(() => setAddress(origin), [origin]);
  useEffect(() => setPem(remote.authority), [remote.authority]);
  useEffect(() => {
    if (session.state === "connected") setEditing(false);
  }, [session.state]);

  const valid = /^https:\/\/[^\s/]+/i.test(address.trim());

  function connect() {
    const next = normalizeOrigin(address);
    if (!fixed) remote.chooseServer(next);
    if (pem.trim() !== remote.authority.trim()) remote.saveAuthority(pem);
    void remote.startLogin();
  }

  if (login) {
    const expired = login.status === "expired" || Date.now() > login.expiresAt;
    return (
      <Card className="connect">
        <div className="connect__step" aria-live="polite">
          <p className="connect__eyebrow">Étape 2 sur 2 · Autoriser cet ordinateur</p>
          <h2>Confirmez ce code dans votre navigateur</h2>
          <p className="connect__code" aria-label="Code de connexion">
            {groupCode(login.userCode)}
          </p>
          <ol className="connect__instructions">
            <li>
              Ouvrez la page d’autorisation de <strong>{hostOf(origin)}</strong> :{" "}
              <a href={login.verificationUri} target="_blank" rel="noreferrer">
                {login.verificationUri} <ExternalLink aria-hidden size={12} />
              </a>
            </li>
            <li>Vérifiez que le code affiché est identique, puis confirmez.</li>
            <li>Revenez ici : Desktop détecte l’autorisation.</li>
          </ol>
          <Button
            size="sm"
            variant="ghost"
            onClick={() =>
              void navigator.clipboard?.writeText(login.verificationUri).then(
                () => toast.show({ tone: "success", title: "Adresse copiée" }),
                () => toast.show({ tone: "warning", title: "Copie impossible", description: "Sélectionnez l’adresse manuellement." }),
              )
            }
          >
            Copier l’adresse de la page
          </Button>
          {expired ? (
            <Banner tone="warning" title="Le code a expiré." actions={<Button size="sm" onClick={connect}>Obtenir un nouveau code</Button>}>
              Aucune connexion n’a été créée. Demandez un nouveau code pour recommencer.
            </Banner>
          ) : login.status === "refused" ? (
            <Banner tone="danger" title="L’autorisation a été refusée." actions={<Button size="sm" onClick={connect}>Recommencer</Button>}>
              Aucun jeton n’a été délivré à cet ordinateur.
            </Banner>
          ) : (
            <p className="connect__waiting" role="status">
              {login.status === "checking" ? <Spinner size={14} /> : null}
              {login.error
                ? "La vérification n’a pas abouti. Réessayez lorsque l’autorisation est confirmée."
                : `En attente de votre autorisation. Vérification automatique toutes les ${login.interval} s, jusqu’à ${timeOf(login.expiresAt)}.`}
            </p>
          )}
          {login.error && <code className="sync-detail">{login.error}</code>}
          <div className="connect__actions">
            <Button onClick={remote.cancelLogin}>Annuler</Button>
            {!expired && login.status !== "refused" && (
              <Button variant="primary" icon={<RefreshCw aria-hidden size={16} />} busy={login.status === "checking"} busyLabel="Vérification…" onClick={() => void remote.pollLogin()}>
                Vérifier maintenant
              </Button>
            )}
          </div>
          <p className="connect__note">Annuler ferme ce parcours ; il ne révoque aucun jeton déjà délivré.</p>
        </div>
      </Card>
    );
  }

  if (session.state === "connected" && !editing) {
    return (
      <Card className="connect">
        <div className="connect__done">
          <CheckCircle2 aria-hidden size={28} className="connect__done-icon" />
          <div>
            <h2>Desktop est connecté</h2>
            <p>
              Serveur <strong>{hostOf(origin)}</strong> · compte <strong>{session.displayName || session.principalId}</strong>. Le jeton reste dans le coffre du
              système.
            </p>
          </div>
        </div>
        <NoticeView notice={remote.notice} onClose={remote.clearNotice} />
        <div className="connect__actions">
          <Button icon={<LogOut aria-hidden size={16} />} onClick={() => void remote.logout()}>
            Se déconnecter
          </Button>
          {!fixed && <Button onClick={() => setEditing(true)}>Changer de serveur</Button>}
          {onDone && (
            <Button variant="primary" onClick={onDone}>
              {doneLabel}
            </Button>
          )}
        </div>
      </Card>
    );
  }

  return (
    <Card className="connect">
      <form
        className="connect__step"
        onSubmit={(event) => {
          event.preventDefault();
          if (valid) connect();
        }}
      >
        <p className="connect__eyebrow">Étape 1 sur 2 · Serveur de votre équipe</p>
        <h2>Connecter Desktop à un serveur Layrs</h2>
        <TextField
          label="Adresse du serveur"
          type="url"
          value={address}
          readOnly={fixed}
          placeholder="https://layrs.monentreprise.fr"
          hint={fixed ? "Cet accès limité a été délivré par ce serveur." : "Adresse HTTPS fournie par votre administrateur."}
          error={address && !valid ? "Indiquez une adresse qui commence par https://." : undefined}
          onChange={(event) => setAddress(event.target.value)}
        />
        {session.state === "signedOut" && origin && <Banner tone="warning" title="Session absente ou expirée.">Reconnectez cet ordinateur pour publier, recevoir ou consulter l’équipe.</Banner>}
        {session.state === "unreachable" && <Banner tone="danger" title="Serveur injoignable ou certificat non reconnu.">Vérifiez l’adresse et le réseau. Pour un certificat d’entreprise, ajoutez son autorité ci-dessous.</Banner>}
        <details className="connect__advanced" open={Boolean(pem) || undefined}>
          <summary>
            <KeyRound aria-hidden size={14} /> Certificat d’entreprise (avancé)
          </summary>
          <p>
            Nécessaire seulement si le certificat du serveur est émis par une autorité privée de votre entreprise. Collez le certificat public de cette
            autorité : il est mémorisé pour ce serveur sur cet ordinateur. Ce n’est pas un secret.
          </p>
          <TextAreaField label="Autorité de certification (PEM)" value={pem} rows={5} placeholder="-----BEGIN CERTIFICATE-----" onChange={(event) => setPem(event.target.value)} />
        </details>
        <NoticeView notice={remote.notice} onClose={remote.clearNotice} />
        <div className="connect__actions">
          {session.state === "connected" && <Button onClick={() => setEditing(false)}>Annuler</Button>}
          <Button type="submit" variant="primary" disabledReason={!valid ? "Indiquez l’adresse HTTPS du serveur." : undefined}>
            Se connecter
          </Button>
        </div>
        <p className="connect__note">
          La connexion se confirme dans votre navigateur, sur la page d’autorisation du serveur. Desktop ne demande pas votre mot de passe et garde le
          jeton dans le coffre du système.
        </p>
      </form>
    </Card>
  );
}
