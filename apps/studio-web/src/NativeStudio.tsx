import { useRef, useState } from "react";
import { NativeReviewSurface, NativeReleaseSurface, NativeAdminSurface, NativeHistorySurface, NativeRemoteProjectFiles, nativeAdminRoute } from "@layrs/ui";
import "@layrs/ui/native.css";

type Bootstrap = { spaceId: string; layers: { record: { layerId: string; name: string }; headStepIds: string[] }[]; markers: { record: { name: string; targetStepId: string } }[]; releases: { record: { version: string; targetStepId: string } }[] };
type Command = { action: string; [key: string]: unknown };

export function NativeStudio() {
  const secret = useRef("");
  const generation = useRef(0);
  const [principal, setPrincipal] = useState("");
  const [login, setLogin] = useState("");
  const [password, setPassword] = useState("");
  const [space, setSpace] = useState("");
  const [data, setData] = useState<Bootstrap | null>(null);
  const [selectedReview,setSelectedReview] = useState<{reviewId:string;version:number}|null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  function clearSession() { secret.current = ""; generation.current++; setPrincipal(""); setData(null); setSelectedReview(null); setBusy(false); }
  async function api<T>(path: string, method = "GET", body?: unknown, anonymous = false): Promise<T> {
    if (location.protocol !== "https:") throw new Error("Ouvrez Studio sur l’adresse HTTPS de votre instance Layrs.");
    const requestGeneration = generation.current;
    const headers: Record<string, string> = { Accept: "application/json" };
    if (!anonymous) {
      if (!secret.current) throw new Error("Connectez-vous à cette instance.");
      headers.Authorization = `Bearer ${secret.current}`;
    }
    if (body !== undefined) headers["Content-Type"] = "application/json";
    const response = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), redirect: "error", credentials: "omit", cache: "no-store", signal: AbortSignal.timeout(120000) });
    if (requestGeneration !== generation.current) throw new Error("La session a changé pendant la lecture.");
    if (response.status === 401) { clearSession(); throw new Error("Session expirée ou révoquée."); }
    const reader = response.body?.getReader();
    if (!reader) throw new Error("Réponse serveur absente.");
    const chunks: Uint8Array[] = []; let size = 0;
    try {
      while (true) {
        const { value, done } = await reader.read(); if (done) break;
        size += value.length; if (size > 4 * 1024 * 1024) throw new Error("Réponse trop volumineuse pour cette vue.");
        chunks.push(value);
      }
    } catch (e) { await reader.cancel(); throw e; }
    const bytes = new Uint8Array(size); let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
    const result = size ? JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)) : {};
    if (requestGeneration !== generation.current) throw new Error("La session a changé pendant la lecture.");
    if (!response.ok) {
      if (response.status === 403) setData(null);
      const code = result?.error?.code;
      throw new Error(typeof code === "string" && /^[a-zA-Z0-9_-]{1,96}$/.test(code) ? code : `Erreur serveur ${response.status}`);
    }
    return result as T;
  }
  async function perform(work: () => Promise<void>) {
    if (busy) return; setBusy(true); setError("");
    try { await work(); } catch (e) { setError(String(e)); } finally { setBusy(false); }
  }
  async function signIn() {
    const session = await api<{ principal_id: string; access_token: string }>("/v3/auth/login", "POST", { login, password }, true);
    secret.current = session.access_token; generation.current++; setPrincipal(session.principal_id); setPassword("");
  }
  async function collaboration<T>(command: Command): Promise<T> {
    if (!data) throw new Error("Ouvrez un projet.");
    const base = `/v3/spaces/${encodeURIComponent(data.spaceId)}`;
    switch (command.action) {
      case "stepHistory": return api<T>(`${base}/history`, "POST", {anchor:command.anchor,cursor:command.cursor,limit:command.limit,path:command.path??null,filter:command.filter??{}});
      case "listReleases": return api<T>(`${base}/releases`);
      case "releasePublications": return api<T>(`${base}/releases/${encodeURIComponent(String(command.release))}/publications`);
      case "publishRelease": return api<T>(`${base}/releases/${encodeURIComponent(String(command.release))}/publications`, "POST", command.request);
      case "revokeRelease": return api<T>(`${base}/releases/${encodeURIComponent(String(command.release))}/publications/${encodeURIComponent(String(command.publication))}/revoke`, "POST", command.request);
      case "listReviews": return api<T>(`${base}/reviews`);
      case "listReviewsPage": {
        const query = new URLSearchParams({ limit: String(command.limit) });
        if (command.cursor) query.set("cursor", String(command.cursor));
        return api<T>(`${base}/reviews?${query}`);
      }
      case "review": return api<T>(`${base}/reviews/${encodeURIComponent(String(command.review))}`);
      case "propose": return api<T>(command.review ? `${base}/reviews/${encodeURIComponent(String(command.review))}/versions` : `${base}/reviews`, "POST", command.request);
      case "decide": return api<T>(`${base}/reviews/${encodeURIComponent(String(command.review))}/actions`, "POST", command.request);
      case "compare": return api<T>(`${base}/compare`, "POST", { before: command.before, after: command.after });
      case "runDetail": return api<T>(`${base}/runs/${encodeURIComponent(String(command.run))}`);
      case "listRuns": return api<T>(`${base}/runs`);
      case "cancelRun": return api<T>(`${base}/runs/${encodeURIComponent(String(command.run))}/cancel`,"POST",command.request);
      case "correctComment": return api<T>(`${base}/reviews/${encodeURIComponent(String(command.review))}/actions/${encodeURIComponent(String(command.comment))}/corrections`,"POST",command.request);
      case "listRunsPage": {
        const query = new URLSearchParams({limit:String(command.limit ?? 50)});
        if (command.cursor) query.set("cursor",String(command.cursor));
        return api<T>(`${base}/runs?${query}`);
      }
      default: throw new Error("Commande de collaboration inconnue.");
    }
  }
  async function administration<T>(command: Command): Promise<T> { const route = nativeAdminRoute(command, data?.spaceId || ""); return api<T>(route.path, route.method, route.body); }
  return <main className="native-workbench"><header><div><p className="native-eyebrow">LAYRS · ÉQUIPE</p><h1>Projets et revues</h1></div>{principal && <button disabled={busy} onClick={() => void perform(async () => { await api("/v3/auth/logout", "POST", {}); clearSession(); })}>Se déconnecter</button>}</header>
    {!principal ? <form className="native-open" onSubmit={e => { e.preventDefault(); void perform(signIn); }}><label>Compte<input value={login} autoComplete="username" disabled={busy} onChange={e => setLogin(e.target.value)} /></label><label>Mot de passe<input type="password" value={password} autoComplete="current-password" disabled={busy} onChange={e => setPassword(e.target.value)} /></label><button type="submit" disabled={busy || !login || !password}>Se connecter</button></form> : <>
      <section className="native-open"><label>Identifiant du projet<input value={space} placeholder="space:…" disabled={busy} onChange={e => setSpace(e.target.value)} /></label><button disabled={busy || !space} onClick={() => void perform(async () => { const next = await api<Bootstrap>(`/v3/spaces/${encodeURIComponent(space)}/bootstrap`); setSelectedReview(null); setData(next); })}>Ouvrir le projet</button></section>
      <NativeAdminSurface onOpenReview={setSelectedReview} key={`admin:${principal}:${data?.spaceId || ""}`} scope={`${principal}:${data?.spaceId || ""}`} transport={administration} busy={busy} onBusy={setBusy} />
      {data && <NativeReleaseSurface key={`release:${principal}:${data.spaceId}`} scope={`${principal}:${data.spaceId}`} transport={collaboration} busy={busy} onBusy={setBusy} />}
      {data && <NativeHistorySurface key={`history:${principal}:${data.spaceId}`} scope={`${principal}:${data.spaceId}`} transport={collaboration} busy={busy} onBusy={setBusy} />}
      {data && <NativeRemoteProjectFiles key={`files:${principal}:${data.spaceId}`} scope={`${location.origin}:${principal}:${data.spaceId}`} busy={busy} onBusy={setBusy}
        loadContext={()=>api(`/v3/spaces/${encodeURIComponent(data.spaceId)}/bootstrap`)}
        loadPage={request=>api(`/v3/spaces/${encodeURIComponent(data.spaceId)}/projection`,"POST",request)} />}
      {data && <><section><h2>Layers et captures publiées</h2>{data.layers.map(layer => <article key={layer.record.layerId}><strong>{layer.record.name}</strong><code>{layer.record.layerId}</code>{layer.headStepIds.map(id => <code key={id}>{id}</code>)}{layer.headStepIds.length > 1 && <p>Plusieurs têtes : intégration nécessaire.</p>}</article>)}<h2>Versions nommées</h2>{data.markers.map((marker, i) => <article key={`marker-${i}`}><strong>{marker.record.name}</strong><code>{marker.record.targetStepId}</code></article>)}{data.releases.map((release, i) => <article key={`release-${i}`}><strong>{release.record.version}</strong><code>{release.record.targetStepId}</code></article>)}</section><NativeReviewSurface spaceId={data.spaceId} selectedReview={selectedReview} draftIdentity={{origin:location.origin,principalId:principal}} key={`${principal}:${data.spaceId}`} scope={`${principal}:${data.spaceId}`} busy={busy} onBusy={setBusy} transport={collaboration} /></>}
    </>}{busy && <p role="status">Opération en cours…</p>}{error && <p className="native-error" role="alert">{error}</p>}
  </main>;
}
