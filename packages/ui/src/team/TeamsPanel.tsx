import { useEffect, useMemo, useRef, useState } from "react";
import { Archive, Clock, Pencil, Plus, Users } from "lucide-react";
import { Button } from "../components/Button";
import { Avatar, Badge, Banner, Card, EmptyState, Loading } from "../components/Feedback";
import { Dialog } from "../components/Dialog";
import { SelectField, TextAreaField, TextField } from "../components/Field";
import { useToast } from "../components/Toast";
import { when } from "./time";
import { describeError } from "./errors";
import { personName, teamRoles, type Person, type Team, type TeamHistoryPage, type TeamRole, type TeamTransport } from "./types";
import type { Directory } from "./useDirectory";

type Editor = { original: Team | null; archive: boolean };

/** P15 · Équipes: composition, role and history of each team. */
export function TeamsPanel({
  directory,
  administration,
  projectName,
  onChanged,
}: {
  directory: Directory;
  administration: TeamTransport;
  projectName: string;
  onChanged?: () => void;
}) {
  const [editor, setEditor] = useState<Editor | null>(null);
  const [history, setHistory] = useState<Team | null>(null);
  const { teams, canManage, canAdminister, loading, error, management } = directory;
  const active = teams.filter((team) => team.state === "active");
  const archived = teams.filter((team) => team.state !== "active");
  const reason = !canAdminister ? "Réservé aux administrateurs du projet." : management?.mode === "custom" ? "Règles personnalisées actives." : undefined;

  if (loading && !teams.length) return <Loading label="Lecture des équipes…" />;
  return (
    <div className="team-panel">
      {error && <Banner tone="danger">{error}</Banner>}
      <div className="team-toolbar">
        <p className="team-muted">
          Une équipe donne le même rôle à tous ses membres dans ce projet. Les accès individuels restent indépendants et un refus explicite reste prioritaire.
        </p>
        <Button variant="primary" icon={<Plus aria-hidden size={16} />} disabledReason={reason} onClick={() => setEditor({ original: null, archive: false })}>
          Nouvelle équipe
        </Button>
      </div>
      {!teams.length ? (
        <EmptyState compact icon={<Users size={22} />} title="Aucune équipe dans ce projet">
          {canManage ? "Créez une équipe pour donner le même rôle à plusieurs membres." : "Les administrateurs du projet peuvent créer des équipes."}
        </EmptyState>
      ) : (
        <div className="team-cards">
          {[...active, ...archived].map((team) => (
            <Card key={team.teamId} className="team-card">
              <div className="team-card__head">
                <div>
                  <h3>{team.name}</h3>
                  <p className="team-row__meta">
                    {teamRoles[team.role]} · {team.memberPrincipalIds.length} membre{team.memberPrincipalIds.length > 1 ? "s" : ""} · version {team.version}
                  </p>
                </div>
                <Badge tone={team.state === "active" ? "success" : "info"}>{team.state === "active" ? "Active" : "Archivée"}</Badge>
              </div>
              {team.memberPrincipalIds.length > 0 && (
                <ul className="team-chips" aria-label={`Membres de ${team.name}`}>
                  {team.memberPrincipalIds.map((id) => (
                    <li key={id} className="team-chip">
                      <Avatar name={personName(directory.people, id)} size={20} />
                      {personName(directory.people, id)}
                    </li>
                  ))}
                </ul>
              )}
              <p className="team-row__meta">
                Modifiée le {when(team.updatedAt)} par {personName(directory.people, team.changedBy)}
                {team.reason ? ` · Motif : ${team.reason}` : ""}
              </p>
              <div className="team-actions">
                {team.state === "active" && (
                  <>
                    <Button size="sm" icon={<Pencil aria-hidden size={14} />} disabledReason={reason} onClick={() => setEditor({ original: team, archive: false })} aria-label={`Modifier l’équipe ${team.name}`}>
                      Modifier
                    </Button>
                    <Button size="sm" variant="ghost" icon={<Archive aria-hidden size={14} />} disabledReason={reason} onClick={() => setEditor({ original: team, archive: true })} aria-label={`Archiver l’équipe ${team.name}`}>
                      Archiver
                    </Button>
                  </>
                )}
                <Button size="sm" variant="ghost" icon={<Clock aria-hidden size={14} />} onClick={() => setHistory(team)} aria-label={`Historique de l’équipe ${team.name}`}>
                  Historique
                </Button>
              </div>
            </Card>
          ))}
        </div>
      )}
      <TeamEditor value={editor} onClose={() => setEditor(null)} directory={directory} administration={administration} projectName={projectName} onChanged={onChanged} />
      <TeamHistory team={history} onClose={() => setHistory(null)} administration={administration} people={directory.people} />
    </div>
  );
}

function TeamEditor({
  value,
  onClose,
  directory,
  administration,
  projectName,
  onChanged,
}: {
  value: Editor | null;
  onClose: () => void;
  directory: Directory;
  administration: TeamTransport;
  projectName: string;
  onChanged?: () => void;
}) {
  const toast = useToast();
  const [name, setName] = useState("");
  const [role, setRole] = useState<TeamRole>("member");
  const [members, setMembers] = useState<string[]>([]);
  const [reason, setReason] = useState("");
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const retry = useRef<{ digest: string; key: string } | null>(null);

  useEffect(() => {
    retry.current = null;
    setError("");
    setQuery("");
    setName(value?.original?.name ?? "");
    setRole(value?.original?.role ?? "member");
    setMembers([...(value?.original?.memberPrincipalIds ?? [])]);
    setReason("");
  }, [value]);

  const original = value?.original ?? null;
  const archive = value?.archive ?? false;
  const unavailable = members.filter((id) => !directory.members.some((member) => member.principalId === id));
  const eligible = useMemo(
    () =>
      [...directory.members]
        .filter((member) => member.displayName.toLocaleLowerCase("fr").includes(query.trim().toLocaleLowerCase("fr")))
        .sort((a, b) => a.displayName.localeCompare(b.displayName, "fr")),
    [directory.members, query],
  );
  const added = members.filter((id) => !original?.memberPrincipalIds.includes(id));
  const removed = (original?.memberPrincipalIds ?? []).filter((id) => !members.includes(id));

  async function save() {
    const body = archive
      ? { expectedVersion: original!.version, reason: reason.trim() }
      : { expectedVersion: original?.version ?? 0, name: name.trim(), role, memberPrincipalIds: [...members].sort() };
    const action = archive ? "archiveTeam" : original ? "updateTeam" : "createTeam";
    const digest = JSON.stringify({ action, team: original?.teamId, body });
    if (retry.current?.digest !== digest) retry.current = { digest, key: `team-${crypto.randomUUID()}` };
    setBusy(true);
    setError("");
    try {
      await administration<{ team: Team }>({ action, team: original?.teamId, request: { ...body, idempotencyKey: retry.current.key } });
      retry.current = null;
      await directory.reload();
      onChanged?.();
      toast.show({
        tone: "success",
        title: archive ? "Équipe archivée" : original ? "Équipe enregistrée" : "Équipe créée",
        description: archive ? "Ses droits sont retirés ; son historique reste consultable." : "Les droits du projet ont été relus sur le serveur.",
      });
      onClose();
    } catch (cause) {
      setError(describeError(cause, archive ? "L’équipe n’a pas été archivée." : "L’équipe n’a pas été enregistrée."));
    } finally {
      setBusy(false);
    }
  }

  const invalid = archive ? (!reason.trim() ? "Indiquez le motif d’archivage." : undefined) : !name.trim() ? "Donnez un nom à l’équipe." : undefined;
  return (
    <Dialog
      open={value !== null}
      onClose={() => !busy && onClose()}
      dismissible={!busy}
      size="lg"
      title={archive ? `Archiver l’équipe ${original?.name}` : original ? `Modifier l’équipe ${original.name}` : "Nouvelle équipe"}
      description={`Projet ${projectName}.${original ? ` Modification de la version ${original.version} : une modification concurrente sera refusée.` : ""}`}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Fermer sans enregistrer
          </Button>
          <Button variant={archive ? "danger" : "primary"} busy={busy} busyLabel="Enregistrement…" disabledReason={invalid} onClick={() => void save()}>
            {archive ? "Archiver l’équipe" : "Enregistrer l’équipe"}
          </Button>
        </>
      }
    >
      <div className="team-form">
        {archive ? (
          <>
            <Banner tone="warning" title="Effets de l’archivage">
              Les droits accordés par cette équipe sont retirés à ses {original?.memberPrincipalIds.length ?? 0} membre(s). Leurs accès individuels et leurs autres équipes sont
              conservés. L’historique reste consultable.
            </Banner>
            <TextAreaField label="Motif d’archivage" required maxLength={4096} value={reason} onChange={(event) => setReason(event.target.value)} />
          </>
        ) : (
          <>
            <div className="team-grid-2">
              <TextField label="Nom de l’équipe" required maxLength={128} value={name} onChange={(event) => setName(event.target.value)} />
              <SelectField label="Rôle de l’équipe" value={role} onChange={(event) => setRole(event.target.value as TeamRole)}>
                {Object.entries(teamRoles).map(([key, label]) => (
                  <option key={key} value={key}>
                    {label}
                  </option>
                ))}
              </SelectField>
            </div>
            <fieldset className="team-picker">
              <legend className="layrs-field__label">Membres ({members.length}/64)</legend>
              <p className="layrs-field__hint">Seuls les comptes humains actifs de l’organisation sont proposés. Invitez d’abord une personne extérieure.</p>
              {directory.members.length > 6 && (
                <TextField label="Rechercher un membre" hideLabel placeholder="Rechercher par nom…" value={query} onChange={(event) => setQuery(event.target.value)} />
              )}
              <ul className="team-picker__list team-picker__list--checks">
                {eligible.map((member) => {
                  const checked = members.includes(member.principalId);
                  return (
                    <li key={member.principalId}>
                      <label className="team-check">
                        <input
                          type="checkbox"
                          checked={checked}
                          disabled={!checked && members.length >= 64}
                          onChange={(event) =>
                            setMembers((current) => (event.target.checked ? [...current, member.principalId] : current.filter((id) => id !== member.principalId)))
                          }
                        />
                        <Avatar name={member.displayName} size={24} />
                        {member.displayName}
                      </label>
                    </li>
                  );
                })}
              </ul>
            </fieldset>
            {unavailable.map((id) => (
              <Banner key={id} tone="warning" actions={<Button size="sm" onClick={() => setMembers((current) => current.filter((item) => item !== id))}>Retirer de l’équipe</Button>}>
                {personName(directory.people, id)} n’est plus un compte actif de l’organisation.
              </Banner>
            ))}
            {original && (added.length > 0 || removed.length > 0 || role !== original.role) && (
              <Banner tone="info" title="Changements prévus">
                {[
                  role !== original.role && `Rôle : ${teamRoles[original.role]} → ${teamRoles[role]}`,
                  added.length > 0 && `Ajout : ${added.map((id) => personName(directory.people, id)).join(", ")}`,
                  removed.length > 0 && `Retrait : ${removed.map((id) => personName(directory.people, id)).join(", ")}`,
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </Banner>
            )}
          </>
        )}
        {error && <Banner tone="danger">{error}</Banner>}
      </div>
    </Dialog>
  );
}

function TeamHistory({ team, onClose, administration, people }: { team: Team | null; onClose: () => void; administration: TeamTransport; people: Map<string, Person> }) {
  const [page, setPage] = useState<TeamHistoryPage | null>(null);
  const [events, setEvents] = useState<TeamHistoryPage["events"]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);

  async function read(next: boolean) {
    if (!team) return;
    const current = ++generation.current;
    setBusy(true);
    setError("");
    try {
      const value = await administration<TeamHistoryPage>({
        action: "teamHistory",
        team: team.teamId,
        after: next ? (page?.nextAfter ?? 0) : 0,
        limit: 20,
        anchor: next ? (page?.anchorVersion ?? null) : null,
      });
      if (current !== generation.current) return;
      setPage(value);
      setEvents((previous) => (next ? [...previous, ...value.events] : value.events));
    } catch (cause) {
      if (current === generation.current) setError(describeError(cause, "L’historique n’a pas pu être lu."));
    } finally {
      if (current === generation.current) setBusy(false);
    }
  }

  useEffect(() => {
    setPage(null);
    setEvents([]);
    if (team) void read(false);
  }, [team]);

  return (
    <Dialog open={team !== null} onClose={onClose} size="lg" title={`Historique · ${team?.name ?? ""}`} description="Chaque version enregistrée de l’équipe, de la plus ancienne à la plus récente.">
      {error && <Banner tone="danger">{error}</Banner>}
      {busy && !events.length ? (
        <Loading label="Lecture de l’historique…" />
      ) : (
        <ol className="team-history" aria-label="Historique de l’équipe">
          {events.map((event) => (
            <li key={event.eventObjectId}>
              <p className="team-row__title">
                Version {event.record.version} · {event.record.name} {event.record.state !== "active" && <Badge>Archivée</Badge>}
              </p>
              <p className="team-row__meta">
                {teamRoles[event.record.role]} · {when(event.record.updatedAt)} · par {personName(people, event.record.changedBy)}
              </p>
              <p className="team-row__meta">Membres : {event.record.memberPrincipalIds.map((id) => personName(people, id)).join(", ") || "aucun"}</p>
              {event.record.reason && <p className="team-row__meta">Motif : {event.record.reason}</p>}
            </li>
          ))}
        </ol>
      )}
      {page?.hasMore && (
        <Button onClick={() => void read(true)} busy={busy} busyLabel="Lecture…">
          Changements suivants
        </Button>
      )}
    </Dialog>
  );
}
