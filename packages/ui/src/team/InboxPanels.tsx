import { useCallback, useEffect, useRef, useState } from "react";
import { Activity, Check, ExternalLink, Mail } from "lucide-react";
import { Button } from "../components/Button";
import { Badge, Banner, EmptyState, Loading } from "../components/Feedback";
import { useToast } from "../components/Toast";
import { ago, when } from "./time";
import { ConfirmDialog, type Confirmation } from "./ConfirmDialog";
import { invitationStatus } from "./InvitationsPanel";
import { eventLabels, memberRoles, type Invitation, type InvitationPage, type NoticePage, type Notice, type TeamTransport } from "./types";
import { useTask } from "./useTask";
import { validReviewContext } from "../native-review-context";

/** P18 · Invitations received by this account, across projects. */
export function InvitationInbox({
  scope,
  administration,
  projectName,
  onOpenProject,
  onDecided,
}: {
  scope: string;
  /** Instance-level transport (no project needed). */
  administration: TeamTransport;
  projectName: (spaceId: string) => string | undefined;
  onOpenProject: (spaceId: string) => void;
  onDecided?: (invitation: Invitation) => void;
}) {
  const task = useTask(scope);
  const toast = useToast();
  const [items, setItems] = useState<Invitation[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [confirm, setConfirm] = useState<Confirmation | null>(null);

  const load = useCallback(
    (after: number | null = null) =>
      task.run("load", async (live) => {
        const page = await administration<InvitationPage>({ action: "invitationInbox", after: after ?? 0 });
        if (!live()) return;
        setItems((current) => (after ? [...(current ?? []), ...page.invitations] : page.invitations));
        setNext(page.nextAfter);
      }, "Vos invitations n’ont pas pu être lues."),
    [administration, task],
  );
  const started = useRef("");
  useEffect(() => {
    if (started.current === scope) return;
    started.current = scope;
    void load();
  }, [scope, load]);

  const name = (item: Invitation) => projectName(item.spaceId) ?? `Projet ${item.spaceId.replace(/^space:/, "").slice(0, 8)}…`;
  async function decide(item: Invitation, decision: "accept" | "reject") {
    await administration<Invitation>({ action: "decideInvitation", invitation: item.invitationId, decision });
    onDecided?.(item);
    if (decision === "accept")
      toast.show({
        tone: "success",
        title: "Invitation acceptée",
        description: `Vous pouvez ouvrir ${name(item)}.`,
        action: { label: "Ouvrir le projet", onSelect: () => onOpenProject(item.spaceId) },
      });
    else toast.show({ tone: "success", title: "Invitation refusée", description: name(item) });
  }

  const pending = (items ?? []).filter((item) => item.state === "pending");
  const others = (items ?? []).filter((item) => item.state !== "pending");
  return (
    <div className="team-panel team-panel--narrow">
      {task.error && <Banner tone="danger">{task.error}</Banner>}
      {!items ? (
        <Loading label="Lecture des invitations…" />
      ) : !items.length ? (
        <EmptyState icon={<Mail size={22} />} title="Aucune invitation">
          Quand un administrateur vous invite à un projet, l’invitation apparaît ici. Aucun courriel n’est envoyé.
        </EmptyState>
      ) : (
        <>
          <h2 className="team-section-title">En attente ({pending.length})</h2>
          {!pending.length && <p className="team-muted">Aucune invitation en attente.</p>}
          <ul className="team-list" aria-label="Invitations en attente">
            {pending.map((item) => {
              const status = invitationStatus(item);
              const usable = !item.expired && !item.stale;
              return (
                <li key={item.invitationId} className="team-row">
                  <Mail aria-hidden size={20} />
                  <div className="team-row__main">
                    <p className="team-row__title">{name(item)}</p>
                    <p className="team-row__meta">
                      Rôle proposé : {memberRoles[item.role as "member"]?.label ?? item.role} · expire le {when(item.expiresAt)}
                    </p>
                    {!usable && <p className="team-row__meta">Cette invitation n’est plus valable ; demandez-en une nouvelle à un administrateur du projet.</p>}
                  </div>
                  <div className="team-row__aside">
                    <Badge tone={status.tone}>{status.label}</Badge>
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={task.busy}
                      onClick={() =>
                        setConfirm({
                          title: "Refuser l’invitation",
                          subject: name(item),
                          effects: ["Vous ne rejoindrez pas ce projet.", "Un administrateur devra vous inviter à nouveau si vous changez d’avis."],
                          confirmLabel: "Refuser",
                          danger: true,
                          run: async () => {
                            await decide(item, "reject");
                            await load();
                          },
                        })
                      }
                    >
                      Refuser
                    </Button>
                    <Button
                      size="sm"
                      variant="primary"
                      icon={<Check aria-hidden size={14} />}
                      busy={task.pending === `accept:${item.invitationId}`}
                      busyLabel="Acceptation…"
                      disabledReason={!usable ? "Invitation expirée ou caduque." : undefined}
                      onClick={() =>
                        void task
                          .run(`accept:${item.invitationId}`, () => decide(item, "accept"), "L’invitation n’a pas été acceptée.")
                          .then((accepted) => accepted && load())
                      }
                    >
                      Accepter
                    </Button>
                  </div>
                </li>
              );
            })}
          </ul>
          {others.length > 0 && (
            <>
              <h2 className="team-section-title">Traitées</h2>
              <ul className="team-list" aria-label="Invitations traitées">
                {others.map((item) => {
                  const status = invitationStatus(item);
                  return (
                    <li key={item.invitationId} className="team-row">
                      <Mail aria-hidden size={20} />
                      <div className="team-row__main">
                        <p className="team-row__title">{name(item)}</p>
                        <p className="team-row__meta">{item.decidedAt ? `Le ${when(item.decidedAt)}` : `Expirait le ${when(item.expiresAt)}`}</p>
                      </div>
                      <div className="team-row__aside">
                        <Badge tone={status.tone}>{status.label}</Badge>
                        {item.state === "accepted" && (
                          <Button size="sm" icon={<ExternalLink aria-hidden size={14} />} onClick={() => onOpenProject(item.spaceId)}>
                            Ouvrir
                          </Button>
                        )}
                      </div>
                    </li>
                  );
                })}
              </ul>
            </>
          )}
          {next !== null && (
            <Button busy={task.pending === "load"} busyLabel="Lecture…" onClick={() => void load(next)}>
              Invitations suivantes
            </Button>
          )}
        </>
      )}
      <ConfirmDialog value={confirm} onClose={() => setConfirm(null)} />
    </div>
  );
}

/** P18 · Activity of the selected project, marked read after server confirmation. */
export function ProjectActivity({
  scope,
  administration,
  projectName,
  onOpenReview,
  describeActor,
}: {
  scope: string;
  /** Transport bound to the selected project. */
  administration: TeamTransport;
  projectName: string;
  onOpenReview?: (context: { reviewId: string; version: number }) => void;
  describeActor?: (principalId: string) => string | undefined;
}) {
  const task = useTask(scope);
  const [items, setItems] = useState<Notice[] | null>(null);
  const [next, setNext] = useState(0);
  const [more, setMore] = useState(false);
  const [lastRead, setLastRead] = useState(0);

  const load = useCallback(
    (after = 0) =>
      task.run("load", async (live) => {
        const page = await administration<NoticePage>({ action: "notifications", after });
        if (!live()) return;
        setItems((current) => (after ? [...(current ?? []), ...page.notifications.filter((item) => !(current ?? []).some((known) => known.sequence === item.sequence))] : page.notifications));
        setNext(page.nextAfter);
        setMore(page.hasMore);
        setLastRead(page.lastReadSequence);
      }, "L’activité du projet n’a pas pu être lue."),
    [administration, task],
  );
  const started = useRef("");
  useEffect(() => {
    if (started.current === scope) return;
    started.current = scope;
    setItems(null);
    void load();
  }, [scope, load]);

  const unread = (items ?? []).filter((item) => item.sequence > lastRead).length;
  const ordered = [...(items ?? [])].reverse();
  return (
    <div className="team-panel team-panel--narrow">
      <div className="team-toolbar">
        <p className="team-muted">
          {items ? `${unread} événement${unread > 1 ? "s" : ""} non lu${unread > 1 ? "s" : ""} parmi ceux affichés pour ${projectName}.` : `Activité de ${projectName}.`}
        </p>
        <Button
          disabledReason={!items?.length ? "Aucun événement chargé." : unread === 0 ? "Tout est déjà lu." : undefined}
          busy={task.pending === "read"}
          busyLabel="Enregistrement…"
          onClick={() =>
            void task.run("read", async (live) => {
              const value = await administration<{ lastReadSequence: number }>({ action: "markNotificationsRead", lastSequence: next });
              if (live()) setLastRead(value.lastReadSequence);
            }, "La lecture n’a pas été enregistrée.")
          }
        >
          Marquer comme lus
        </Button>
      </div>
      {task.error && <Banner tone="danger">{task.error}</Banner>}
      {!items ? (
        <Loading label="Lecture de l’activité…" />
      ) : !items.length ? (
        <EmptyState icon={<Activity size={22} />} title="Aucun événement">
          Les revues, validations et changements d’accès de ce projet apparaîtront ici.
        </EmptyState>
      ) : (
        <ul className="team-list" aria-label={`Activité de ${projectName}`}>
          {ordered.map((item) => {
            const fresh = item.sequence > lastRead;
            const actor = item.actorPrincipalId ? describeActor?.(item.actorPrincipalId) : undefined;
            return (
              <li key={item.sequence} className={["team-row", fresh && "team-row--unread"].filter(Boolean).join(" ")}>
                <Activity aria-hidden size={18} />
                <div className="team-row__main">
                  <p className="team-row__title">
                    {eventLabels[item.kind] ?? item.kind}
                    {fresh && <Badge tone="accent">Non lu</Badge>}
                  </p>
                  <p className="team-row__meta">
                    {ago(item.createdAt)}
                    {actor && ` · ${actor}`}
                  </p>
                </div>
                {validReviewContext(item.reviewContext) && onOpenReview && (
                  <div className="team-row__aside">
                    <Button size="sm" icon={<ExternalLink aria-hidden size={14} />} onClick={() => onOpenReview({ reviewId: item.reviewContext!.reviewId, version: item.reviewContext!.version })}>
                      Ouvrir la revue · version {item.reviewContext!.version}
                    </Button>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {more && (
        <Button busy={task.pending === "load"} busyLabel="Lecture…" onClick={() => void load(next)}>
          Événements suivants
        </Button>
      )}
    </div>
  );
}
