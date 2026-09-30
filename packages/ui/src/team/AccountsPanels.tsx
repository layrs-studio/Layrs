import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Bot, KeyRound, Plus, UserPlus, Users } from "lucide-react";
import { Button } from "../components/Button";
import { Avatar, Badge, Banner, EmptyState, Loading } from "../components/Feedback";
import { Dialog } from "../components/Dialog";
import { PasswordField, SelectField, TextField } from "../components/Field";
import { useToast } from "../components/Toast";
import { ConfirmDialog, type Confirmation } from "./ConfirmDialog";
import { describeError } from "./errors";
import { SecretReveal, type RevealedSecret } from "./SecretReveal";
import { accountKinds, type Account, type TeamTransport } from "./types";
import { useTask } from "./useTask";

const LOGIN = /^[a-z0-9@._+-]{3,254}$/;

function useAccounts(scope: string, administration: TeamTransport) {
  const task = useTask(scope);
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [more, setMore] = useState(false);
  const load = useCallback(
    () =>
      task.run("load", async (live) => {
        const value = await administration<{ accounts: Account[]; hasMore?: boolean }>({ action: "listAccounts" });
        if (!live()) return;
        setAccounts(value.accounts);
        setMore(Boolean(value.hasMore));
      }, "Les comptes n’ont pas pu être lus."),
    [administration, task],
  );
  const started = useRef("");
  useEffect(() => {
    if (started.current === scope) return;
    started.current = scope;
    void load();
  }, [scope, load]);
  return { task, accounts, more, load };
}

const statusLabel = (status: string) => (status === "active" ? "Actif" : status === "revoked" ? "Révoqué" : status);

/** P21 · Comptes: human accounts of the instance. */
export function AccountsPanel({ scope, administration, selfId }: { scope: string; administration: TeamTransport; selfId: string }) {
  const { task, accounts, more, load } = useAccounts(scope, administration);
  const [creating, setCreating] = useState(false);
  const [confirm, setConfirm] = useState<Confirmation | null>(null);
  const [query, setQuery] = useState("");
  const toast = useToast();
  const humans = useMemo(
    () =>
      (accounts ?? [])
        .filter((account) => account.kind !== "service")
        .filter((account) => account.displayName.toLocaleLowerCase("fr").includes(query.trim().toLocaleLowerCase("fr"))),
    [accounts, query],
  );
  return (
    <div className="team-panel team-panel--narrow">
      <div className="team-toolbar">
        <p className="team-muted">Un compte donne accès à l’instance ; les droits sur un projet s’obtiennent ensuite par invitation ou accès direct.</p>
        <Button variant="primary" icon={<UserPlus aria-hidden size={16} />} onClick={() => setCreating(true)}>
          Créer un compte
        </Button>
      </div>
      {task.error && <Banner tone="danger">{task.error}</Banner>}
      {more && <Banner tone="info">Seuls les 256 premiers comptes sont listés.</Banner>}
      {!accounts ? (
        <Loading label="Lecture des comptes…" />
      ) : (
        <>
          {accounts.length > 8 && <TextField label="Rechercher un compte" hideLabel placeholder="Rechercher par nom…" value={query} onChange={(event) => setQuery(event.target.value)} />}
          {!humans.length ? (
            <EmptyState compact icon={<Users size={22} />} title="Aucun compte ne correspond" />
          ) : (
            <ul className="team-list" aria-label="Comptes de l’instance">
              {humans.map((account) => (
                <li key={account.principalId} className="team-row">
                  <Avatar name={account.displayName} size={32} />
                  <div className="team-row__main">
                    <p className="team-row__title">
                      {account.displayName}
                      {account.principalId === selfId && <Badge>Vous</Badge>}
                    </p>
                    <p className="team-row__meta">{accountKinds[account.kind] ?? account.kind}</p>
                  </div>
                  <div className="team-row__aside">
                    <Badge tone={account.status === "active" ? "success" : "danger"}>{statusLabel(account.status)}</Badge>
                    {account.status === "active" && account.kind !== "bootstrap_admin" && (
                      <Button
                        size="sm"
                        variant="ghost"
                        aria-label={`Révoquer le compte ${account.displayName}`}
                        onClick={() =>
                          setConfirm({
                            title: "Révoquer le compte",
                            subject: `${account.displayName} · toute l’instance`,
                            effects: [
                              "Toutes ses sessions et jetons sont révoqués immédiatement ; la personne ne peut plus se connecter.",
                              "Elle perd l’usage de tous ses projets ; ses contributions passées restent dans l’historique.",
                              "La révocation est définitive : aucune réactivation n’existe, il faudrait créer un nouveau compte.",
                            ],
                            confirmLabel: "Révoquer le compte",
                            danger: true,
                            run: async () => {
                              await administration({ action: "revokeAccount", principalId: account.principalId });
                              toast.show({ tone: "success", title: "Compte révoqué", description: account.displayName });
                              await load();
                            },
                          })
                        }
                      >
                        Révoquer
                      </Button>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
      <CreateAccount open={creating} onClose={() => setCreating(false)} administration={administration} onCreated={() => void load()} />
      <ConfirmDialog value={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}

function CreateAccount({ open, onClose, administration, onCreated }: { open: boolean; onClose: () => void; administration: TeamTransport; onCreated: () => void }) {
  const toast = useToast();
  const [displayName, setDisplayName] = useState("");
  const [login, setLogin] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    setDisplayName("");
    setLogin("");
    setPassword("");
    setError("");
  }, [open]);
  // The initial password never lingers in memory.
  useEffect(() => {
    if (!password) return;
    const timer = window.setTimeout(() => setPassword(""), 60_000);
    return () => window.clearTimeout(timer);
  }, [password]);
  const normalized = login.trim().toLowerCase();
  const invalid = !displayName.trim()
    ? "Indiquez le nom affiché."
    : !LOGIN.test(normalized)
      ? "Identifiant de 3 à 254 caractères : lettres, chiffres, @ . _ + -"
      : password.length < 12
        ? "Mot de passe initial de 12 caractères au moins."
        : undefined;

  async function submit() {
    setBusy(true);
    setError("");
    try {
      await administration({ action: "createAccount", request: { login: normalized, displayName: displayName.trim(), password } });
      toast.show({ tone: "success", title: "Compte créé", description: `${displayName.trim()} · transmettez le mot de passe initial par votre canal sécurisé habituel.` });
      onCreated();
      onClose();
    } catch (cause) {
      setError(describeError(cause, "Le compte n’a pas été créé (identifiant peut-être déjà utilisé)."));
    } finally {
      setPassword("");
      setBusy(false);
    }
  }

  return (
    <Dialog
      open={open}
      onClose={() => !busy && onClose()}
      dismissible={!busy}
      title="Créer un compte"
      description="Compte humain de l’instance. Il ne donne accès à aucun projet tant qu’il n’y est pas invité."
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Annuler
          </Button>
          <Button variant="primary" busy={busy} busyLabel="Création…" disabledReason={invalid} onClick={() => void submit()}>
            Créer le compte
          </Button>
        </>
      }
    >
      <form className="team-form" autoComplete="off" onSubmit={(event) => event.preventDefault()}>
        <TextField label="Nom affiché" maxLength={128} value={displayName} onChange={(event) => setDisplayName(event.target.value)} />
        <TextField label="Identifiant de connexion" maxLength={254} autoComplete="off" spellCheck={false} value={login} onChange={(event) => setLogin(event.target.value)} />
        <PasswordField
          label="Mot de passe initial"
          hint="12 caractères au moins. Effacé du formulaire après 60 secondes ; la personne pourra le changer depuis son compte."
          autoComplete="new-password"
          maxLength={1024}
          value={password}
          onChange={(event) => setPassword(event.target.value)}
        />
        {error && <Banner tone="danger">{error}</Banner>}
      </form>
    </Dialog>
  );
}

/** P21 · Comptes de service: technical identities and their tokens. */
export function ServiceAccountsPanel({ scope, administration }: { scope: string; administration: TeamTransport }) {
  const { task, accounts, load } = useAccounts(scope, administration);
  const [form, setForm] = useState<{ service: Account | null } | null>(null);
  const [confirm, setConfirm] = useState<Confirmation | null>(null);
  const [secret, setSecret] = useState<RevealedSecret | null>(null);
  const clear = useCallback(() => setSecret(null), []);
  const toast = useToast();
  const services = (accounts ?? []).filter((account) => account.kind === "service");
  useEffect(() => setSecret(null), [scope]);
  return (
    <div className="team-panel team-panel--narrow">
      <div className="team-toolbar">
        <p className="team-muted">
          Un compte de service représente un outil (CI, intégration). Il n’hérite d’aucun droit : donnez-lui ensuite un rôle dans chaque projet concerné.
        </p>
        <Button variant="primary" icon={<Plus aria-hidden size={16} />} onClick={() => setForm({ service: null })}>
          Créer un compte de service
        </Button>
      </div>
      {task.error && <Banner tone="danger">{task.error}</Banner>}
      {!accounts ? (
        <Loading label="Lecture des comptes de service…" />
      ) : !services.length ? (
        <EmptyState compact icon={<Bot size={22} />} title="Aucun compte de service" />
      ) : (
        <ul className="team-list" aria-label="Comptes de service">
          {services.map((service) => (
            <li key={service.principalId} className="team-row">
              <Bot aria-hidden size={20} />
              <div className="team-row__main">
                <p className="team-row__title">{service.displayName}</p>
                <p className="team-row__meta">Compte de service</p>
              </div>
              <div className="team-row__aside">
                <Badge tone={service.status === "active" ? "success" : "danger"}>{statusLabel(service.status)}</Badge>
                {service.status === "active" && (
                  <>
                    <Button size="sm" icon={<KeyRound aria-hidden size={14} />} onClick={() => setForm({ service })} aria-label={`Renouveler le jeton de ${service.displayName}`}>
                      Renouveler le jeton
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      aria-label={`Révoquer ${service.displayName}`}
                      onClick={() =>
                        setConfirm({
                          title: "Révoquer le compte de service",
                          subject: `${service.displayName} · toute l’instance`,
                          effects: ["Tous ses jetons sont révoqués immédiatement ; l’outil qui l’utilise cessera de fonctionner.", "La révocation est définitive."],
                          confirmLabel: "Révoquer",
                          danger: true,
                          run: async () => {
                            await administration({ action: "revokeAccount", principalId: service.principalId });
                            toast.show({ tone: "success", title: "Compte de service révoqué", description: service.displayName });
                            await load();
                          },
                        })
                      }
                    >
                      Révoquer
                    </Button>
                  </>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
      <ServiceTokenDialog
        value={form}
        onClose={() => setForm(null)}
        onPrepared={(service, request) => {
          if (!service) {
            setForm(null);
            return void task
              .run("create", async (live) => {
                const value = await administration<{ principal_id: string; access_token: string }>({ action: "createService", request });
                if (!live()) return;
                setSecret({
                  title: "Jeton du compte de service",
                  owner: `${String(request.displayName)} · jeton « ${String(request.label)} »`,
                  guidance: "Enregistrez-le dans le coffre de l’outil concerné.",
                  value: value.access_token,
                });
              }, "Le compte de service n’a pas été créé.")
              .then((created) => created && load());
          }
          setForm(null);
          setConfirm({
            title: "Renouveler le jeton",
            subject: `${service.displayName} · jeton « ${String(request.label)} »`,
            effects: [
              "Tous les anciens jetons de ce compte de service sont révoqués immédiatement.",
              "L’outil qui l’utilise doit recevoir le nouveau jeton, affiché une seule fois.",
            ],
            confirmLabel: "Renouveler le jeton",
            danger: true,
            run: async () => {
              const value = await administration<{ access_token: string }>({ action: "rotateService", request: { principalId: service.principalId, label: request.label, expiresIn: request.expiresIn } });
              setSecret({
                title: "Nouveau jeton du compte de service",
                owner: `${service.displayName} · jeton « ${String(request.label)} »`,
                guidance: "Remplacez l’ancien jeton dans le coffre de l’outil concerné.",
                value: value.access_token,
              });
            },
          });
        }}
      />
      <SecretReveal secret={secret} onDone={clear} />
      <ConfirmDialog value={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}

function ServiceTokenDialog({
  value,
  onClose,
  onPrepared,
}: {
  value: { service: Account | null } | null;
  onClose: () => void;
  onPrepared: (service: Account | null, request: { displayName?: string; label: string; expiresIn: number }) => void;
}) {
  const [name, setName] = useState("");
  const [label, setLabel] = useState("");
  const [days, setDays] = useState(30);
  useEffect(() => {
    setName("");
    setLabel("");
    setDays(30);
  }, [value]);
  const service = value?.service ?? null;
  const invalid = !service && !name.trim() ? "Nommez le compte de service." : !label.trim() ? "Nommez le jeton (ex. : outil et usage)." : undefined;
  return (
    <Dialog
      open={value !== null}
      onClose={onClose}
      title={service ? `Renouveler le jeton de ${service.displayName}` : "Créer un compte de service"}
      description="Le jeton sera affiché une seule fois."
      footer={
        <>
          <Button onClick={onClose}>Annuler</Button>
          <Button
            variant="primary"
            disabledReason={invalid}
            onClick={() => onPrepared(service, { ...(service ? {} : { displayName: name.trim() }), label: label.trim(), expiresIn: days * 86_400 })}
          >
            {service ? "Continuer" : "Créer et afficher le jeton"}
          </Button>
        </>
      }
    >
      <div className="team-form">
        {!service && <TextField label="Nom du compte de service" maxLength={128} value={name} onChange={(event) => setName(event.target.value)} placeholder="CI du projet Atelier" />}
        <TextField label="Nom du jeton" maxLength={128} value={label} onChange={(event) => setLabel(event.target.value)} placeholder="runner principal" />
        <SelectField label="Validité" value={days} onChange={(event) => setDays(Number(event.target.value))}>
          {[1, 7, 30, 60, 90].map((value) => (
            <option key={value} value={value}>
              {value} jour{value > 1 ? "s" : ""}
            </option>
          ))}
        </SelectField>
      </div>
    </Dialog>
  );
}
