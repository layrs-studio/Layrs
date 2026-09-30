import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Mail, UserPlus } from "lucide-react";
import { Button } from "../components/Button";
import { Avatar, Badge, Banner, EmptyState, Loading } from "../components/Feedback";
import { Dialog } from "../components/Dialog";
import { SelectField } from "../components/Field";
import { useToast } from "../components/Toast";
import { ago, when } from "./time";
import { ConfirmDialog, type Confirmation } from "./ConfirmDialog";
import { describeError } from "./errors";
import { PersonPicker } from "./PersonPicker";
import { invitationStates, personName, type Invitation, type InvitationPage, type Person, type TeamTransport } from "./types";
import type { Directory } from "./useDirectory";
import { useTask } from "./useTask";

export function invitationStatus(item: Invitation): { label: string; tone: "info" | "success" | "warning" | "danger" } {
  if (item.state === "pending" && item.expired) return { label: "Expirée", tone: "warning" };
  if (item.state === "pending" && item.stale) return { label: "Caduque : droits du projet modifiés", tone: "warning" };
  if (item.state === "accepted") return { label: invitationStates.accepted, tone: "success" };
  if (item.state === "pending") return { label: invitationStates.pending, tone: "info" };
  return { label: invitationStates[item.state] ?? item.state, tone: item.state === "revoked" ? "danger" : "info" };
}

/** P15 · Invitations sent for this project, and inviting by name. */
export function InvitationsPanel({
  scope,
  directory,
  administration,
  projectName,
  self,
  inviteRequested,
  onInviteHandled,
}: {
  scope: string;
  directory: Directory;
  administration: TeamTransport;
  projectName: string;
  self: Person | null;
  /** Opens the invitation form (e.g. from the Members tab). */
  inviteRequested?: boolean;
  onInviteHandled?: () => void;
}) {
  const task = useTask(scope);
  const [items, setItems] = useState<Invitation[]>([]);
  const [next, setNext] = useState<number | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [inviting, setInviting] = useState(false);
  const [confirm, setConfirm] = useState<Confirmation | null>(null);
  const toast = useToast();
  const { canManage, canAdminister, management } = directory;

  const load = useCallback(
    (more = false) =>
      task.run("load", async (live) => {
        const page = await administration<InvitationPage>({ action: "listInvitations", after: more ? (next ?? 0) : 0 });
        if (!live()) return;
        setItems((current) => (more ? [...current, ...page.invitations.filter((item) => !current.some((known) => known.invitationId === item.invitationId))] : page.invitations));
        setNext(page.nextAfter);
        setLoaded(true);
      }, "Les invitations n’ont pas pu être lues."),
    [administration, next, task],
  );

  const started = useRef("");
  useEffect(() => {
    if (!canAdminister || started.current === scope) return;
    started.current = scope;
    void load();
  }, [scope, canAdminister, load]);

  useEffect(() => {
    if (inviteRequested) {
      setInviting(true);
      onInviteHandled?.();
    }
  }, [inviteRequested, onInviteHandled]);

  if (!canAdminister)
    return (
      <EmptyState icon={<Mail size={22} />} title="Invitations gérées par les administrateurs du projet">
        Vos propres invitations reçues sont dans la boîte de réception.
      </EmptyState>
    );

  const pending = items.filter((item) => item.state === "pending" && !item.expired && !item.stale);
  const sorted = [...items].reverse();
  return (
    <div className="team-panel">
      {management?.mode === "custom" && (
        <Banner tone="warning" title="Invitations suspendues">
          Des règles d’accès personnalisées sont actives. Une invitation acceptée n’accorderait aucun rôle tant que la gestion par équipes n’est pas rétablie.
        </Banner>
      )}
      <div className="team-toolbar">
        <p className="team-muted">
          Aucun courriel n’est envoyé : la personne trouve l’invitation dans sa boîte de réception Layrs après connexion. Elle n’obtient ses droits qu’en l’acceptant.
          {loaded && ` ${pending.length} invitation${pending.length > 1 ? "s" : ""} en attente.`}
        </p>
        <Button variant="primary" icon={<UserPlus aria-hidden size={16} />} onClick={() => setInviting(true)} disabledReason={!canManage ? "Règles personnalisées actives." : undefined}>
          Inviter une personne
        </Button>
      </div>
      {task.error && <Banner tone="danger">{task.error}</Banner>}
      {!loaded && task.pending === "load" ? (
        <Loading label="Lecture des invitations…" />
      ) : loaded && !items.length ? (
        <EmptyState compact icon={<Mail size={22} />} title="Aucune invitation pour ce projet">
          Invitez un compte existant de l’instance. La création d’un compte appartient à l’administration de l’instance.
        </EmptyState>
      ) : (
        <ul className="team-list" aria-label="Invitations du projet">
          {sorted.map((item) => {
            const status = invitationStatus(item);
            const name = personName(directory.people, item.recipientPrincipalId);
            return (
              <li key={item.invitationId} className="team-row">
                <Avatar name={name} size={36} />
                <div className="team-row__main">
                  <p className="team-row__title">{name}</p>
                  <p className="team-row__meta">
                    {item.role === "auditor" ? "Lecture seule" : "Membre"} · invitée par {personName(directory.people, item.inviterPrincipalId)}
                    {item.createdAt && ` · ${ago(item.createdAt)}`}
                  </p>
                  <p className="team-row__meta">
                    {item.state === "pending" ? `Expire le ${when(item.expiresAt)}` : item.decidedAt ? `Décision le ${when(item.decidedAt)}` : `Expirait le ${when(item.expiresAt)}`}
                  </p>
                </div>
                <div className="team-row__aside">
                  <Badge tone={status.tone}>{status.label}</Badge>
                  {item.state === "pending" && (
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() =>
                        setConfirm({
                          title: "Révoquer l’invitation",
                          subject: `${name} · projet ${projectName}`,
                          effects: [
                            "L’invitation ne pourra plus être acceptée.",
                            "Aucun droit déjà accordé n’est retiré : si la personne a déjà accès, changez son rôle dans Membres.",
                          ],
                          confirmLabel: "Révoquer l’invitation",
                          danger: true,
                          run: async () => {
                            await administration({ action: "decideInvitation", invitation: item.invitationId, decision: "revoke" });
                            toast.show({ tone: "success", title: "Invitation révoquée", description: name });
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
            );
          })}
        </ul>
      )}
      {next !== null && (
        <Button onClick={() => void load(true)} busy={task.pending === "load"} busyLabel="Lecture…">
          Invitations suivantes
        </Button>
      )}
      <InviteDialog
        open={inviting}
        onClose={() => setInviting(false)}
        directory={directory}
        administration={administration}
        projectName={projectName}
        self={self}
        onCreated={() => void load()}
      />
      <ConfirmDialog value={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}

function InviteDialog({
  open,
  onClose,
  directory,
  administration,
  projectName,
  self,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  directory: Directory;
  administration: TeamTransport;
  projectName: string;
  self: Person | null;
  onCreated: () => void;
}) {
  const toast = useToast();
  const [recipient, setRecipient] = useState("");
  const [role, setRole] = useState<"member" | "auditor">("member");
  const [days, setDays] = useState(7);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  // The same form content reuses its key, so a lost answer is retried safely.
  const pending = useRef({ signature: "", key: "" });
  const candidates = useMemo(() => {
    const map = new Map<string, Person>();
    for (const account of directory.accounts) if (account.status === "active" && account.kind !== "service") map.set(account.principalId, account);
    for (const member of directory.members) map.set(member.principalId, member);
    return [...map.values()];
  }, [directory.accounts, directory.members]);

  useEffect(() => {
    if (!open) return;
    setRecipient("");
    setRole("member");
    setDays(7);
    setError("");
  }, [open]);

  const name = recipient ? personName(directory.people, recipient) : "";
  const expires = new Date(Date.now() + days * 86_400_000);

  async function submit() {
    const payload = { recipientPrincipalId: recipient, role, expiresInSeconds: days * 86_400 };
    const signature = JSON.stringify(payload);
    if (pending.current.signature !== signature) pending.current = { signature, key: `invite-${crypto.randomUUID()}` };
    setBusy(true);
    setError("");
    try {
      const created = await administration<Invitation>({ action: "inviteMember", request: { ...payload, idempotencyKey: pending.current.key } });
      pending.current = { signature: "", key: "" };
      toast.show({
        tone: "success",
        title: "Invitation créée",
        description: `${name} · expire le ${when(created.expiresAt ?? expires.toISOString())}. La personne doit l’accepter depuis sa boîte de réception.`,
      });
      onCreated();
      onClose();
    } catch (cause) {
      setError(describeError(cause, "L’invitation n’a pas été créée."));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      open={open}
      onClose={() => !busy && onClose()}
      dismissible={!busy}
      size="lg"
      title="Inviter une personne"
      description={`Projet ${projectName}. La personne doit déjà avoir un compte sur cette instance.`}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Annuler
          </Button>
          <Button variant="primary" busy={busy} busyLabel="Création…" disabledReason={!recipient ? "Choisissez la personne à inviter." : undefined} onClick={() => void submit()}>
            Créer l’invitation
          </Button>
        </>
      }
    >
      <div className="team-form">
        <PersonPicker
          label="Personne à inviter"
          people={candidates}
          value={recipient}
          onChange={setRecipient}
          exclude={self ? [self.principalId] : []}
          hint={
            directory.accounts.length
              ? "Comptes actifs de l’instance (visibles car vous administrez l’instance)."
              : "Membres actifs de l’organisation de ce projet. Pour une personne extérieure, utilisez son identifiant technique ci-dessous."
          }
          emptyText="Aucun compte proposé : utilisez l’identifiant technique de la personne."
        />
        <div className="team-grid-2">
          <SelectField label="Droits proposés" value={role} onChange={(event) => setRole(event.target.value as "member" | "auditor")}>
            <option value="member">Membre : consulter, publier et participer aux revues</option>
            <option value="auditor">Lecture seule : consulter le projet et les revues</option>
          </SelectField>
          <SelectField label="Expiration" value={days} onChange={(event) => setDays(Number(event.target.value))} hint={`Jusqu’au ${when(expires.toISOString())} environ.`}>
            {[1, 2, 3, 5, 7].map((value) => (
              <option key={value} value={value}>
                {value} jour{value > 1 ? "s" : ""}
              </option>
            ))}
          </SelectField>
        </div>
        {recipient && (
          <Banner tone="info" title="Ce qui va se passer">
            {name} verra l’invitation dans sa boîte de réception. En l’acceptant avant expiration, elle obtiendra le rôle « {role === "member" ? "Membre" : "Lecture seule"} » sur{" "}
            {projectName}. Si les droits du projet changent d’ici là, l’invitation devient caduque et devra être renouvelée.
          </Banner>
        )}
        {error && <Banner tone="danger">{error}</Banner>}
      </div>
    </Dialog>
  );
}
