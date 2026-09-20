import { useEffect, useRef, useState } from "react";
import { nativeWorkspace } from "./tauri";

type Identity = { actorId: string; deviceId: string | null; displayName: string | null };
export function NativeIdentityPanel({ root, disabled }: { root: string; disabled: boolean }) {
  const [identity, setIdentity] = useState<Identity | null>(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(true);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState("");
  const generation = useRef(0);
  const running = useRef(false);
  useEffect(() => {
    const current = ++generation.current;
    setBusy(true); setReady(false); setIdentity(null); setName(""); setError("");
    nativeWorkspace<{ identity: Identity | null }>(root, { action: "localIdentity" })
      .then(result => { if (generation.current === current) { setIdentity(result.identity); setName(result.identity?.displayName || ""); setReady(true); } })
      .catch(e => { if (generation.current === current) setError(String(e)); })
      .finally(() => { if (generation.current === current) setBusy(false); });
    return () => { generation.current++; };
  }, [root]);
  async function save() {
    if (running.current || !ready) return;
    running.current = true; setBusy(true); setError("");
    const current = generation.current;
    try {
      const next: Identity = { actorId: identity?.actorId || `actor:${crypto.randomUUID()}`, deviceId: identity?.deviceId || `device:${crypto.randomUUID()}`, displayName: name.trim() };
      const result = await nativeWorkspace<{ identity: Identity }>(root, { action: "configureIdentity", identity: next });
      if (generation.current === current) setIdentity(result.identity);
    } catch (e) { if (generation.current === current) setError(String(e)); }
    finally { running.current = false; if (generation.current === current) setBusy(false); }
  }
  return <details><summary>Identité des captures locales</summary>
    <p>Cette identité déclarée attribue vos prochaines captures. Les anciennes observations restent inchangées ; votre compte serveur garde ses propres droits.</p>
    <label>Nom affiché<input value={name} maxLength={256} disabled={disabled || busy} onChange={e => setName(e.target.value)} /></label>
    <button disabled={disabled || busy || !ready || !name.trim()} onClick={() => void save()}>Enregistrer l’identité locale</button>
    {identity && <p>Identifiant pour les filtres d’historique : <code>{identity.actorId}</code></p>}
    {error && <p className="native-error" role="alert">{error}</p>}
  </details>;
}
