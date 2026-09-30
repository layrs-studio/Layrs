import { useMemo, useState } from "react";
import { ShieldCheck, UserCog, UserPlus, Users } from "lucide-react";
import { Button } from "../components/Button";
import { Avatar, Badge, Banner, EmptyState, Loading } from "../components/Feedback";
import { Dialog } from "../components/Dialog";
import { useToast } from "../components/Toast";
import { roleSummary } from "./access-model";
import { describeError } from "./errors";
import { PersonPicker } from "./PersonPicker";
import { memberRoles, type MemberRole, type Person, type TeamTransport } from "./types";
import type { Directory } from "./useDirectory";

type Grant = { principalId: string; kind: string; current: string | null; direct: boolean };

function rolesFor(kind: string): MemberRole[] {
  return kind === "service" ? ["automation", "auditor", "release_manager", "revoked"] : ["member", "auditor", "release_manager", "revoked"];
}

/** P15 · Membres: who works here, with which role, and role changes. */
export function MembersPanel({
  directory,
  administration,
  projectName,
  self,
  onInvite,
  onChanged,
}: {
  directory: Directory;
  administration: TeamTransport;
  projectName: string;
  self: Person | null;
  onInvite?: () => void;
  /** Called after a confirmed change, e.g. to refetch capabilities. */
  onChanged?: () => void;
}) {
  const [grant, setGrant] = useState<Grant | null>(null);
  const { members, policies, management, canAdminister, loading, error, rulesError, teams } = directory;
  const custom = management?.mode === "custom";

  const rows = useMemo(
    () =>
      [...members]
        .sort((a, b) => a.displayName.localeCompare(b.displayName, "fr"))
        .map((member) => ({
          member,
          summary: policies ? roleSummary(policies, member.principalId) : null,
          teams: teams.filter((team) => team.state === "active" && team.memberPrincipalIds.includes(member.principalId)).map((team) => team.name),
        })),
    [members, policies, teams],
  );

  if (loading && !members.length) return <Loading label="Lecture des membres…" />;
  if (!canAdminister)
    return (
      <EmptyState icon={<Users size={22} />} title="Liste des membres réservée aux administrateurs du projet">
        {error ? <Banner tone="danger">{error}</Banner> : "Votre compte peut consulter les équipes et vos propres accès (Accès aux fichiers), mais pas la liste nominative des membres."}
      </EmptyState>
    );

  return (
    <div className="team-panel">
      {custom && (
        <Banner tone="warning" title="Rôles suspendus">
          Ce projet utilise des règles d’accès personnalisées. Rétablissez la gestion par équipes et rôles dans Réglages → Avancé avant de modifier un rôle.
        </Banner>
      )}
      {rulesError && <Banner tone="warning">{rulesError} Les rôles ne sont pas affichés ; les modifications restent possibles.</Banner>}
      <div className="team-toolbar">
        <p className="team-muted">
          {members.length} membre{members.length > 1 ? "s" : ""} actif{members.length > 1 ? "s" : ""} dans l’organisation de ce projet. Le rôle indiqué est celui lu dans les règles actuelles
          du projet.
        </p>
        <div className="team-actions">
          <Button icon={<ShieldCheck aria-hidden size={16} />} onClick={() => setGrant({ principalId: "", kind: "account", current: null, direct: true })} disabledReason={custom ? "Règles personnalisées actives." : undefined}>
            Accorder un accès direct
          </Button>
          {onInvite && (
            <Button variant="primary" icon={<UserPlus aria-hidden size={16} />} onClick={onInvite}>
              Inviter une personne
            </Button>
          )}
        </div>
      </div>
      <ul className="team-list" aria-label="Membres du projet">
        {rows.map(({ member, summary, teams: memberTeams }) => {
          const isSelf = member.principalId === self?.principalId;
          return (
            <li key={member.principalId} className="team-row">
              <Avatar name={member.displayName} size={36} />
              <div className="team-row__main">
                <p className="team-row__title">
                  {member.displayName}
                  {isSelf && <Badge>Vous</Badge>}
                </p>
                <p className="team-row__meta">
                  {summary ? (
                    <>
                      <strong>{summary.label}</strong>
                      {summary.capabilities.length > 0 && ` · ${summary.capabilities.join(", ")}`}
                    </>
                  ) : (
                    "Rôle non lu"
                  )}
                </p>
                {(memberTeams.length > 0 || (summary?.sources.length ?? 0) > 0) && (
                  <p className="team-row__meta">
                    {memberTeams.length > 0 && `Équipes : ${memberTeams.join(", ")}`}
                    {memberTeams.length > 0 && summary?.sources.length ? " · " : ""}
                    {summary?.sources.length ? `Règles : ${[...new Set(summary.sources)].join(", ")}` : ""}
                  </p>
                )}
              </div>
              <div className="team-row__aside">
                {summary?.denied && <Badge tone="danger">Accès retiré</Badge>}
                <Button
                  size="sm"
                  icon={<UserCog aria-hidden size={14} />}
                  disabledReason={custom ? "Règles personnalisées actives." : isSelf ? "Vous ne pouvez pas modifier votre propre rôle." : undefined}
                  onClick={() => setGrant({ principalId: member.principalId, kind: member.kind ?? "account", current: summary?.role ?? null, direct: false })}
                  aria-label={`Changer le rôle de ${member.displayName}`}
                >
                  Changer le rôle
                </Button>
              </div>
            </li>
          );
        })}
      </ul>
      <GrantDialog
        value={grant}
        onClose={() => setGrant(null)}
        directory={directory}
        administration={administration}
        projectName={projectName}
        self={self}
        onChanged={onChanged}
      />
    </div>
  );
}

function GrantDialog({
  value,
  onClose,
  directory,
  administration,
  projectName,
  self,
  onChanged,
}: {
  value: Grant | null;
  onClose: () => void;
  directory: Directory;
  administration: TeamTransport;
  projectName: string;
  self: Person | null;
  onChanged?: () => void;
}) {
  const toast = useToast();
  const [principalId, setPrincipalId] = useState("");
  const [role, setRole] = useState<MemberRole | "">("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [opened, setOpened] = useState<Grant | null>(null);
  if (value !== opened) {
    setOpened(value);
    setPrincipalId(value?.principalId ?? "");
    setRole("");
    setError("");
  }
  const candidates: Person[] = useMemo(() => {
    const map = new Map<string, Person>();
    for (const account of directory.accounts) if (account.status === "active" && account.kind !== "bootstrap_admin") map.set(account.principalId, account);
    for (const member of directory.members) map.set(member.principalId, member);
    return [...map.values()];
  }, [directory.accounts, directory.members]);
  const person = directory.people.get(principalId);
  const kind = person?.kind ?? value?.kind ?? "account";
  const name = person?.displayName ?? (principalId ? `Compte ${principalId.slice(10, 18)}…` : "");
  const chosen = role ? memberRoles[role] : null;

  async function submit() {
    if (!role || !principalId) return;
    setBusy(true);
    setError("");
    try {
      await administration({ action: "grantMember", request: { principalId, role } });
      await directory.reload();
      onChanged?.();
      toast.show({
        tone: "success",
        title: role === "revoked" ? "Accès retiré" : "Rôle appliqué",
        description: `${name} · ${memberRoles[role].label}. Les droits ont été relus sur le serveur.`,
      });
      onClose();
    } catch (cause) {
      setError(describeError(cause, "Le rôle n’a pas été modifié."));
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
      title={value?.direct ? "Accorder un accès direct" : `Changer le rôle de ${name}`}
      description={`Projet ${projectName}. Le changement prend effet pour les prochaines actions de la personne.`}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Annuler
          </Button>
          <Button
            variant={role === "revoked" ? "danger" : "primary"}
            busy={busy}
            busyLabel="Application…"
            disabledReason={!principalId ? "Choisissez une personne." : !role ? "Choisissez un rôle." : role === value?.current ? "C’est déjà son rôle." : undefined}
            onClick={() => void submit()}
          >
            {role === "revoked" ? "Retirer l’accès" : "Appliquer ce rôle"}
          </Button>
        </>
      }
    >
      <div className="team-form">
        {value?.direct && (
          <>
            <p className="team-muted">
              L’accès direct ajoute la personne au projet sans invitation à accepter. Préférez une invitation pour une personne qui doit confirmer sa participation.
            </p>
            <PersonPicker
              label="Personne"
              people={candidates}
              value={principalId}
              onChange={setPrincipalId}
              exclude={self ? [self.principalId] : []}
              hint={directory.accounts.length ? "Comptes actifs de l’instance." : "Membres actifs de l’organisation du projet."}
            />
          </>
        )}
        <fieldset className="team-choices">
          <legend className="layrs-field__label">Rôle dans le projet</legend>
          {rolesFor(kind).map((option) => (
            <label key={option} className={["team-choice", option === "revoked" && "team-choice--danger"].filter(Boolean).join(" ")}>
              <input type="radio" name="member-role" value={option} checked={role === option} onChange={() => setRole(option)} />
              <span>
                <strong>
                  {memberRoles[option].label}
                  {option === value?.current && " (rôle actuel)"}
                </strong>
                <small>{memberRoles[option].description}</small>
              </span>
            </label>
          ))}
        </fieldset>
        {chosen && principalId && (
          <Banner tone={role === "revoked" ? "warning" : "info"} title="Ce qui va se passer">
            {role === "revoked"
              ? `${name} ne pourra plus ouvrir ce projet, même si une équipe lui donne un rôle. Les copies déjà téléchargées ne sont pas effacées.`
              : `${name} aura le rôle « ${chosen.label} » : ${chosen.description} Les validations obtenues sous l’ancienne politique deviennent obsolètes.`}
          </Banner>
        )}
        {error && <Banner tone="danger">{error}</Banner>}
      </div>
    </Dialog>
  );
}
