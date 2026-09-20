import { useEffect, useRef, useState } from "react";
type Command = { action: string; [key: string]: unknown };
type Invitation = { invitationId: string; spaceId: string; inviterPrincipalId: string; recipientPrincipalId: string; role: string; state: string; expiresAt: string; expired: boolean; stale: boolean };
type Page = { invitations: Invitation[]; nextAfter: number | null };

export function NativeInvitationsSurface({ scope, transport, busy, onBusy }: { scope: string; transport: <T>(command: Command) => Promise<T>; busy: boolean; onBusy: (busy: boolean) => void }) {
  const [items, setItems] = useState<Invitation[]>([]);
  const [sent, setSent] = useState(false);
  const [after, setAfter] = useState<number | null>(null);
  const [recipient, setRecipient] = useState("");
  const [role, setRole] = useState("member");
  const [days, setDays] = useState(7);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const generation = useRef(0), active = useRef(false), pending = useRef({ signature: "", key: "" });
  useEffect(() => { generation.current++; setItems([]); setAfter(null); setRecipient(""); setNotice(""); setError(""); pending.current = { signature: "", key: "" }; return () => { generation.current++; }; }, [scope]);
  async function request<T>(command: Command): Promise<T> { const current = generation.current; const result = await transport<T>(command); if (generation.current !== current) throw new Error("La session a changé."); return result; }
  async function run(work: () => Promise<void>) { if (busy || active.current) return; active.current = true; onBusy(true); setError(""); const current = generation.current; try { await work(); } catch (e) { if (current === generation.current) { setItems([]); setAfter(null); setError(String(e)); } } finally { active.current = false; onBusy(false); } }
  async function load(outgoing: boolean, next = false) { const result = await request<Page>({ action: outgoing ? "listInvitations" : "invitationInbox", after: next ? after : 0 }); setItems(result.invitations); setAfter(result.nextAfter); setSent(outgoing); }
  async function create() {
    const payload = { recipientPrincipalId: recipient, role, expiresInSeconds: days * 86400 };
    const signature = JSON.stringify(payload); if (pending.current.signature !== signature) pending.current = { signature, key: crypto.randomUUID() };
    await request({ action: "inviteMember", request: { ...payload, idempotencyKey: pending.current.key } });
    pending.current = { signature: "", key: "" }; setNotice("Invitation créée. Le compte doit l’accepter avant d’obtenir les droits."); await load(true);
  }
  async function decide(invitation: string, decision: string) { await request({ action: "decideInvitation", invitation, decision }); setNotice(decision === "accept" ? "Invitation acceptée. Vous pouvez ouvrir le projet indiqué." : "Décision enregistrée."); await load(sent); }
  const states: Record<string, string> = { pending: "En attente", accepted: "Acceptée", rejected: "Refusée", revoked: "Révoquée" };
  return <details><summary>Invitations aux projets</summary>
    <p>Invitez un compte humain déjà créé sur cette instance. Aucun courriel n’est envoyé ; le destinataire consulte ses invitations après connexion.</p>
    <div className="native-buttons"><button disabled={busy} onClick={() => void run(() => load(false))}>Mes invitations reçues</button><button disabled={busy} onClick={() => void run(() => load(true))}>Invitations envoyées du projet</button><button disabled={busy || after === null} onClick={() => void run(() => load(sent, true))}>Invitations suivantes</button></div>
    <details><summary>Inviter un compte au projet ouvert</summary><label>Compte à inviter (identifiant)<input value={recipient} onChange={e => setRecipient(e.target.value)} placeholder="principal:…" /></label><label>Droits proposés<select value={role} onChange={e => setRole(e.target.value)}><option value="member">Membre : consulter, publier et participer aux revues</option><option value="auditor">Lecture : consulter le projet et les revues</option></select></label><label>Expiration dans combien de jours<input type="number" min={1} max={7} value={days} onChange={e => setDays(Number(e.target.value))} /></label><button disabled={busy || !recipient || !Number.isInteger(days) || days < 1 || days > 7} onClick={() => void run(create)}>Créer l’invitation</button></details>
    {items.map(item => <article key={item.invitationId}><strong>{states[item.state] || item.state} · {item.role === "member" ? "Membre" : "Lecture"}</strong><p>Projet : <code>{item.spaceId}</code></p><p>{sent ? "Destinataire" : "Invité par"} : <code>{sent ? item.recipientPrincipalId : item.inviterPrincipalId}</code></p><p>Expire le {item.expiresAt}</p>{item.state === "pending" && (item.expired || item.stale) && <p>Invitation expirée ou droits modifiés : demandez une nouvelle invitation.</p>}{item.state === "pending" && <div className="native-buttons">{sent ? <button disabled={busy} onClick={() => void run(() => decide(item.invitationId, "revoke"))}>Révoquer cette invitation</button> : <><button disabled={busy || item.expired || item.stale} onClick={() => void run(() => decide(item.invitationId, "accept"))}>Accepter l’invitation</button><button disabled={busy} onClick={() => void run(() => decide(item.invitationId, "reject"))}>Refuser l’invitation</button></>}</div>}</article>)}
    {notice && <p role="status">{notice}</p>}{error && <p role="alert">{error}</p>}
  </details>;
}
