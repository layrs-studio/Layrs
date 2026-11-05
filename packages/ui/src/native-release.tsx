import { useEffect, useRef, useState } from "react";

type Release = { record: { releaseId: string; version: string; targetStepId: string } };
type Event = { eventObjectId: string; event: { kind: "published" | "revoked"; occurredAt: string; actorId: string; reason?: string; publicationId?: string } };
type Publications = { releaseId: string; lastEventObjectId: string | null; activePublicationId: string | null; events: Event[] };
type Command = { action: string; [key: string]: unknown };

export function NativeReleaseSurface({ scope, transport, busy, onBusy }: {
  scope: string; transport: <T>(command: Command) => Promise<T>; busy: boolean; onBusy: (value: boolean) => void;
}) {
  const [releases, setReleases] = useState<Release[]>([]);
  const [selected, setSelected] = useState("");
  const [history, setHistory] = useState<Publications | null>(null);
  const [reason, setReason] = useState("");
  const [error, setError] = useState("");
  const generation = useRef(0);
  const inFlight = useRef(false);
  const pending = useRef({ signature: "", requestId: "" });
  useEffect(() => {
    generation.current++; setReleases([]); setSelected(""); setHistory(null); setReason(""); setError("");
    pending.current = { signature: "", requestId: "" };
    return () => { generation.current++; };
  }, [scope]);
  async function request<T>(command: Command) {
    const current = generation.current;
    const result = await transport<T>(command);
    if (generation.current !== current) throw new Error("Le projet ou la session a changé.");
    return result;
  }
  async function perform(work: () => Promise<void>) {
    if (busy || inFlight.current) return;
    inFlight.current = true; onBusy(true); setError("");
    const current = generation.current;
    try { await work(); }
    catch (e) { if (generation.current === current) { setError(String(e)); setHistory(null); setReleases([]); } }
    finally { inFlight.current = false; onBusy(false); }
  }
  async function load(release: string) {
    const value = await request<Publications>({ action: "releasePublications", release });
    setSelected(release); setHistory(value);
  }
  async function mutate(revoke: boolean) {
    if (!history || history.releaseId !== selected) return;
    const command = { action: revoke ? "revokeRelease" : "publishRelease", release: selected, ...(revoke ? { publication: history.activePublicationId } : {}) };
    const body = { expectedLastEventId: history.lastEventObjectId, ...(revoke ? { reason } : {}) };
    const signature = JSON.stringify({ command, body });
    if (pending.current.signature !== signature) pending.current = { signature, requestId: crypto.randomUUID() };
    await request({ ...command, request: { ...body, requestId: pending.current.requestId } });
    pending.current = { signature: "", requestId: "" }; setReason(""); await load(selected);
  }
  return <section aria-label="Publications des versions"><h2>Publier une version</h2>
    <p>La publication désigne une version exacte. Une révocation conserve son manifeste, ses fichiers et son historique.</p>
    <button disabled={busy} onClick={() => void perform(async () => {
      const response = await request<{ releases: Release[] }>({ action: "listReleases" });
      setReleases(response.releases); setHistory(null); setSelected("");
    })}>Charger les versions distantes</button>
    <label>Version distante<select disabled={busy || !releases.length} value={selected} onChange={e => { const id = e.target.value; setSelected(id); setHistory(null); if (id) void perform(() => load(id)); }}>
      <option value="">Choisir une version</option>{releases.map(({ record }) => <option key={record.releaseId} value={record.releaseId}>{record.version} · {record.releaseId}</option>)}
    </select></label>
    {history && <><p>{history.activePublicationId ? "Version publiée" : "Aucune publication active"}</p>
      <code>{releases.find(r => r.record.releaseId === selected)?.record.targetStepId}</code>
      {history.activePublicationId ? <><label>Motif de révocation<textarea disabled={busy} value={reason} maxLength={4096} onChange={e => setReason(e.target.value)} /></label>
        <button disabled={busy || !reason.trim()} onClick={() => void perform(() => mutate(true))}>Révoquer cette publication</button></>
        : <button disabled={busy} onClick={() => void perform(() => mutate(false))}>Publier cette version</button>}
      <ol>{history.events.map(entry => <li key={entry.eventObjectId}><strong>{entry.event.kind === "published" ? "Publication" : "Révocation"}</strong> <time>{entry.event.occurredAt}</time><code>{entry.event.actorId}</code>{entry.event.reason && <p>{entry.event.reason}</p>}</li>)}</ol>
    </>}
    {error && <p className="native-error" role="alert">{error}</p>}
  </section>;
}
