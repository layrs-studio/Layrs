import { useEffect, useRef, useState } from "react";
import type { NativeAdminCommand } from "./native-admin-route";
import { NativePolicyManagement, type PolicyManagement } from "./native-policy-management";

type Role = "member" | "auditor" | "release_manager";
type Team = { teamId: string; version: number; name: string; role: Role; memberPrincipalIds: string[]; state: string; eventObjectId: string; updatedAt: string; changedBy: string; reason?: string };
type Member = { principalId: string; displayName: string };
type Editor = { original: Team | null; name: string; role: Role; members: string[]; archive: boolean; reason: string };
type History = { events: { eventObjectId: string; record: Team }[]; anchorVersion: number; nextAfter: number; hasMore: boolean };
const roles: Record<Role, string> = { member: "Membre", auditor: "Lecture et audit", release_manager: "Publication des versions" };

export function NativeTeamsSurface({ scope, transport, busy, onBusy, onPolicy, globalPolicy }: {
  scope: string; transport: <T>(command: NativeAdminCommand) => Promise<T>; busy: boolean; onBusy: (busy: boolean) => void;
  onPolicy: (value: PolicyManagement) => void;
  globalPolicy: PolicyManagement | null;
}) {
  const [policy, setPolicy] = useState<PolicyManagement | null>(null);
  const [teams, setTeams] = useState<Team[]>([]);
  const [members, setMembers] = useState<Member[]>([]);
  const [canManage, setCanManage] = useState(false);
  const [editor, setEditor] = useState<Editor | null>(null);
  const [history, setHistory] = useState<{team: Team; page: History} | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const generation = useRef(0);
  const inFlight = useRef(false);
  const retry = useRef<{digest: string; key: string} | null>(null);
  useEffect(() => {
    if (globalPolicy && globalPolicy.currentPolicySetId !== policy?.currentPolicySetId) {
      setPolicy(globalPolicy); setCanManage(false); setEditor(null); retry.current = null;
    }
  }, [globalPolicy, policy?.currentPolicySetId]);
  useEffect(() => {
    generation.current++; inFlight.current = false; retry.current = null;
    setTeams([]); setMembers([]); setCanManage(false); setPolicy(null); setEditor(null); setHistory(null); setError(""); setNotice("");
    return () => { generation.current++; };
  }, [scope]);
  async function request<T>(command: NativeAdminCommand): Promise<T> {
    const current = generation.current;
    const result = await transport<T>(command);
    if (current !== generation.current) throw new Error("Le projet ou la session a changé.");
    return result;
  }
  async function perform(work: () => Promise<void>) {
    if (busy || inFlight.current) return;
    const current = generation.current;
    inFlight.current = true; onBusy(true); setError(""); setNotice("");
    try { await work(); }
    catch (e) { if (current === generation.current) setError(String(e)); }
    finally { if (current === generation.current) { inFlight.current = false; onBusy(false); } }
  }
  async function load() {
    const current = generation.current;
    setTeams([]); setMembers([]); setCanManage(false); setHistory(null); setEditor(null); setPolicy(null);
    const list = await request<{teams: Team[]; policyManagement: PolicyManagement}>({action:"listTeams"});
    setTeams(list.teams);
    setPolicy(list.policyManagement); onPolicy(list.policyManagement);
    if (list.policyManagement.mode === "custom") return;
    // Listing teams is readable independently of the administrator-only directory.
    try { setMembers((await request<{members: Member[]}>({action:"eligibleTeamMembers"})).members); setCanManage(true); }
    catch { if (current === generation.current) setNotice("La liste des membres administrables est indisponible. Vérifiez vos droits et la connexion."); }
  }
  function edit(team: Team | null, archive = false) {
    retry.current = null; setError(""); setNotice("");
    setEditor({original:team,name:team?.name || "",role:team?.role || "member",members:[...(team?.memberPrincipalIds || [])],archive,reason:""});
  }
  async function save() {
    if (!editor || !canManage || policy?.mode !== "managed") return;
    const original = editor.original;
    const body = editor.archive ? {expectedVersion:original!.version,reason:editor.reason} : {
      expectedVersion:original?.version || 0,name:editor.name,role:editor.role,memberPrincipalIds:[...editor.members].sort(),
    };
    const action = editor.archive ? "archiveTeam" : original ? "updateTeam" : "createTeam";
    const digest = JSON.stringify({action,team:original?.teamId,body});
    if (retry.current?.digest !== digest) retry.current = {digest,key:crypto.randomUUID()};
    const result = await request<{team: Team}>({action,team:original?.teamId,request:{...body,idempotencyKey:retry.current.key}});
    setTeams(previous => [...previous.filter(team => team.teamId !== result.team.teamId),result.team].sort((a,b)=>a.name.localeCompare(b.name)));
    retry.current = null; setEditor(null); setHistory(null);
    setNotice(editor.archive ? "Équipe archivée. Ses droits ont été retirés ; l’historique reste consultable." : "Équipe enregistrée. Les droits du projet sont à jour.");
  }
  async function readHistory(team: Team, next = false) {
    const page = await request<History>({action:"teamHistory",team:team.teamId,after:next ? history?.page.nextAfter || 0 : 0,limit:20,anchor:next ? history?.page.anchorVersion : null});
    setHistory({team,page});
  }
  const memberName = (id: string) => members.find(member => member.principalId === id)?.displayName || id;
  return <details><summary>Équipes du projet</summary>
    <NativePolicyManagement value={policy} />
    <p>Une équipe attribue le même rôle à ses membres dans ce projet. Les accès individuels restent indépendants ; une interdiction explicite reste prioritaire.</p>
    <div className="native-buttons"><button disabled={busy} onClick={() => void perform(load)}>Charger les équipes</button><button disabled={busy || !canManage} onClick={() => edit(null)}>Nouvelle équipe</button></div>
    <section aria-label="Équipes du projet">{teams.map(team => <article key={team.teamId}>
      <h3>{team.name}</h3><p>{roles[team.role]} · version {team.version} · {team.state === "archived" ? "Archivée" : "Active"}</p>
      <ul>{team.memberPrincipalIds.map(id => <li key={id}>{memberName(id)}</li>)}</ul>
      <button disabled={busy || team.state !== "active" || !canManage} onClick={() => edit(team)}>Modifier cette équipe</button>
      <button disabled={busy || team.state !== "active" || !canManage} onClick={() => edit(team,true)}>Préparer l’archivage</button>
      <button disabled={busy} onClick={() => void perform(()=>readHistory(team))}>Historique de cette équipe</button>
    </article>)}</section>
    {editor && <form aria-label="Modifier les droits d’une équipe" onSubmit={e => {e.preventDefault(); void perform(save);}}>
      <h3>{editor.archive ? `Archiver ${editor.name}` : editor.original ? `Modifier ${editor.original.name}` : "Créer une équipe"}</h3>
      {editor.archive ? <><p>Cette action retire les droits accordés par cette équipe. Les accès individuels et les autres équipes sont conservés.</p><label>Motif d’archivage<textarea required maxLength={4096} disabled={busy} value={editor.reason} onChange={e=>setEditor({...editor,reason:e.target.value})} /></label></> : <>
        <label>Nom de l’équipe<input required maxLength={128} disabled={busy} value={editor.name} onChange={e=>setEditor({...editor,name:e.target.value})} /></label>
        <label>Rôle de l’équipe<select disabled={busy} value={editor.role} onChange={e=>setEditor({...editor,role:e.target.value as Role})}>{Object.entries(roles).map(([value,label])=><option key={value} value={value}>{label}</option>)}</select></label>
        <fieldset disabled={busy}><legend>Membres de l’équipe ({editor.members.length}/64)</legend>{members.map(member=><label className="native-check" key={member.principalId}><input type="checkbox" checked={editor.members.includes(member.principalId)} disabled={!editor.members.includes(member.principalId) && editor.members.length>=64} onChange={e=>setEditor({...editor,members:e.target.checked ? [...editor.members,member.principalId] : editor.members.filter(id=>id!==member.principalId)})} />{member.displayName}</label>)}</fieldset>
        {editor.members.filter(id=>!members.some(member=>member.principalId===id)).map(id=><p key={id}>Compte devenu indisponible : <code>{id}</code><button type="button" disabled={busy} onClick={()=>setEditor({...editor,members:editor.members.filter(member=>member!==id)})}>Retirer ce compte</button></p>)}
        <p>Seuls les comptes humains actifs déjà membres de l’organisation sont proposés. Utilisez une invitation pour ajouter un nouveau compte.</p>
      </>}
      {editor.original && <p>Modification de la version {editor.original.version}. Une modification concurrente sera refusée.</p>}
      <button type="submit" disabled={busy || (editor.archive ? !editor.reason.trim() : !editor.name.trim())}>{editor.archive ? "Archiver l’équipe" : "Enregistrer l’équipe"}</button>
      <button type="button" disabled={busy} onClick={()=>{setEditor(null);retry.current=null;}}>Fermer sans enregistrer</button>
    </form>}
    {history && <section aria-label="Historique de l’équipe"><h3>Historique · {history.team.name}</h3><ol>{history.page.events.map(event=><li key={event.eventObjectId}>
      <strong>Version {event.record.version} · {event.record.name}</strong><p>{roles[event.record.role]} · {event.record.state} · {event.record.updatedAt}</p>
      <p>Modifié par <code>{event.record.changedBy}</code></p><ul>{event.record.memberPrincipalIds.map(id=><li key={id}>{memberName(id)}</li>)}</ul>{event.record.reason && <p>{event.record.reason}</p>}
      <details><summary>Identifiant de l’événement</summary><code>{event.eventObjectId}</code></details>
    </li>)}</ol><button disabled={busy || !history.page.hasMore} onClick={()=>void perform(()=>readHistory(history.team,true))}>Page suivante des changements d’équipe</button></section>}
    {notice && <p role="status">{notice}</p>}{error && <p role="alert" className="native-error">{error}</p>}
  </details>;
}
