import { useEffect, useRef, useState } from "react";
import type { TeamRole, TeamTransport } from "./types";
import { teamRoles } from "./types";
import { isForbidden } from "./errors";

type Person = { principalId: string; displayName: string };
type OrganisationTeam = {
  teamId: string;
  version: number;
  name: string;
  memberPrincipalIds: string[];
  state: "active" | "archived";
  assignment: { role: TeamRole; version: number } | null;
  assignmentVersion: number;
};
type Catalogue = { workspaceId: string; teams: OrganisationTeam[]; eligibleMembers: Person[]; canManageDirectory: boolean };
type Editor = { team: OrganisationTeam | null; name: string; members: string[] };

/** Organisation directory and explicit grants for the project currently open. */
export function OrganisationTeamsPanel({ scope, administration, onChanged }: { scope: string; administration: TeamTransport; onChanged?: () => void }) {
  const [catalogue, setCatalogue] = useState<Catalogue | null>(null);
  const [visible, setVisible] = useState(true);
  const [editor, setEditor] = useState<Editor | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const retry = useRef<{ digest: string; key: string } | null>(null);
  const generation = useRef(0);
  const admin = useRef(administration);
  admin.current = administration;
  async function refresh(live: number) {
    try {
      const value = await admin.current<Catalogue>({ action: "listOrganisationTeams" });
      if (generation.current === live) { setCatalogue(value); setVisible(true); setError(""); }
    } catch (cause) {
      if (generation.current !== live) return;
      if (isForbidden(cause)) setVisible(false);
      else setError(String(cause));
    }
  }
  useEffect(() => {
    const live = ++generation.current;
    setCatalogue(null); setEditor(null); setError(""); setNotice(""); setVisible(true);
    void refresh(live);
    return () => { generation.current++; };
  }, [scope]);
  async function mutate(action: string, team: string | undefined, request: Record<string, unknown>, success: string) {
    if (busy) return;
    const digest = JSON.stringify({ action, team, request });
    if (retry.current?.digest !== digest) retry.current = { digest, key: crypto.randomUUID() };
    const live = generation.current;
    const transport = admin.current;
    setBusy(true); setError(""); setNotice("");
    try {
      await transport({ action, team, request: { ...request, idempotencyKey: retry.current.key } });
      if (generation.current !== live) return;
      retry.current = null;
      setEditor(null);
      await refresh(live);
      if (generation.current === live) onChanged?.();
      setNotice(success);
    } catch (cause) {
      if (generation.current === live) setError(String(cause));
    } finally {
      if (generation.current === live) setBusy(false);
    }
  }
  if (!visible) return null;
  const names = new Map(catalogue?.eligibleMembers.map(person => [person.principalId, person.displayName]));
  return <section aria-label="Équipes de l’organisation" className="team-panel">
    <h2 className="team-section-title">Équipes de l’organisation</h2>
    <p className="team-muted">Une équipe se compose au niveau de l’organisation. Son rôle est attribué séparément dans chaque projet.</p>
    {error && <p role="alert" className="native-error">{error}</p>}
    {notice && <p role="status">{notice}</p>}
    {!catalogue ? <p>Chargement des équipes…</p> : <>
      {catalogue.canManageDirectory && <button type="button" disabled={busy} onClick={() => { retry.current = null; setEditor({ team: null, name: "", members: [] }); }}>Créer une équipe de l’organisation</button>}
      {catalogue.teams.length === 0 && <p>Aucune équipe dans cette organisation.</p>}
      <div className="team-cards">{catalogue.teams.map(team => <article className="team-card" key={team.teamId}>
        <h3>{team.name}</h3>
        <p>{team.memberPrincipalIds.length} membre{team.memberPrincipalIds.length > 1 ? "s" : ""} · version {team.version} · {team.state === "archived" ? "Archivée" : "Active"}</p>
        <ul>{team.memberPrincipalIds.map(id => <li key={id}>{names.get(id) ?? id}</li>)}</ul>
        {team.state === "active" && <>
          <label>Rôle dans ce projet
            <select disabled={busy} value={team.assignment?.role ?? ""} onChange={event => void mutate("assignOrganisationTeam", team.teamId, { expectedVersion: team.assignmentVersion, role: event.target.value || null }, "Droits du projet mis à jour.")}>
              <option value="">Aucun accès par cette équipe</option>
              {Object.entries(teamRoles).map(([role, label]) => <option key={role} value={role}>{label}</option>)}
            </select>
          </label>
          {catalogue.canManageDirectory && <div className="team-actions">
            <button type="button" disabled={busy} onClick={() => { retry.current = null; setEditor({ team, name: team.name, members: [...team.memberPrincipalIds] }); }}>Modifier les membres</button>
            <button type="button" disabled={busy} onClick={() => {
              const reason = window.prompt(`Motif d’archivage pour ${team.name}`);
              if (reason?.trim()) void mutate("archiveOrganisationTeam", team.teamId, { expectedVersion: team.version, reason: reason.trim() }, "Équipe archivée ; ses droits ont été retirés des projets liés.");
            }}>Archiver</button>
          </div>}
        </>}
      </article>)}</div>
    </>}
    {editor && catalogue && <form className="team-form" aria-label="Modifier une équipe de l’organisation" onSubmit={event => {
      event.preventDefault();
      void mutate(editor.team ? "updateOrganisationTeam" : "createOrganisationTeam", editor.team?.teamId, {
        expectedVersion: editor.team?.version ?? 0, name: editor.name.trim(), memberPrincipalIds: [...editor.members].sort(),
      }, "Équipe enregistrée ; tous ses projets liés sont à jour.");
    }}>
      <h3>{editor.team ? `Modifier ${editor.team.name}` : "Nouvelle équipe"}</h3>
      <label>Nom de l’équipe<input required maxLength={128} disabled={busy} value={editor.name} onChange={event => setEditor({ ...editor, name: event.target.value })} /></label>
      <fieldset disabled={busy}><legend>Membres ({editor.members.length}/64)</legend>
        {catalogue.eligibleMembers.map(person => <label key={person.principalId} className="native-check"><input type="checkbox" checked={editor.members.includes(person.principalId)} disabled={!editor.members.includes(person.principalId) && editor.members.length >= 64} onChange={event => setEditor({ ...editor, members: event.target.checked ? [...editor.members, person.principalId] : editor.members.filter(id => id !== person.principalId) })} />{person.displayName}</label>)}
      </fieldset>
      {editor.members.filter(id => !names.has(id)).map(id => <p key={id}>Compte indisponible : {id} <button type="button" onClick={() => setEditor({ ...editor, members: editor.members.filter(member => member !== id) })}>Retirer</button></p>)}
      <button type="submit" disabled={busy || !editor.name.trim()}>Enregistrer</button>
      <button type="button" disabled={busy} onClick={() => { retry.current = null; setEditor(null); }}>Annuler</button>
    </form>}
  </section>;
}
