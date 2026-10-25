import { useCallback, useEffect, useState } from "react";
import { Banner, EmptyState } from "../components/Feedback";
import { Button } from "../components/Button";
import type { LensCatalog, LensConfiguration, LensPacket, LensTransport } from "./types";
import { pinKey } from "./types";
import { useLensRuntime } from "./runtime";
import { ConfirmDialog, type Confirmation } from "../team/ConfirmDialog";

export function InstalledLensesPanel({ transport, scope }: { transport: LensTransport; scope: string }) {
  const [catalog, setCatalog] = useState<LensCatalog | null>(null);
  const [packet, setPacket] = useState<LensPacket | null>(null);
  const [approved, setApproved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const refresh = useCallback(async () => {
    setCatalog(await transport<LensCatalog>({ action: "listLenses" }));
  }, [transport]);
  useEffect(() => { let alive = true;
    setCatalog(null); setPacket(null); setError("");
    transport<LensCatalog>({ action: "listLenses" }).then(value => { if (alive) setCatalog(value); }, () => { if (alive) setError("Le catalogue n’a pas pu être lu."); });
    return () => { alive = false; };
  }, [transport, scope]);
  async function read(file?: File) {
    setPacket(null); setApproved(false); setNotice(""); setError("");
    if (!file) return;
    try {
      if (file.size > 12 * 1024 * 1024) throw Error("Paquet trop volumineux (12 Mio maximum).");
      const value = JSON.parse(await file.text()) as LensPacket;
      if (!value?.manifest?.id || !value.manifest.version || !Array.isArray(value.files)) throw Error("Paquet .layrslens.json invalide.");
      setPacket(value);
      setApproved(Boolean(value.manifest.bundle?.signature));
    } catch (cause) { setError(cause instanceof Error ? cause.message : "Lecture impossible."); }
  }
  async function install() {
    if (!packet || !approved || busy) return;
    setBusy(true); setError(""); setNotice("");
    try {
      await transport({ action: "installLens", request: { ...packet, approveUnsigned: !packet.manifest.bundle?.signature && approved } });
      setPacket(null); setApproved(false); await refresh();
      setNotice("Lens installée. Vous pouvez maintenant l’activer dans les réglages d’un projet.");
    } catch (cause) { setError(cause instanceof Error ? cause.message : "L’installation a échoué."); }
    finally { setBusy(false); }
  }
  return <section className="team-panel" aria-label="Lenses installées">
    <h2>Lenses de l’instance</h2>
    <p>Installez les composants sur le serveur, puis choisissez les versions utilisées dans chaque projet. Les clients les récupèrent automatiquement.</p>
    {error && <Banner tone="danger">{error}</Banner>}
    {notice && <Banner tone="success">{notice}</Banner>}
    <label>Installer une lens <input type="file" accept=".json,.layrslens" disabled={busy} onChange={event => void read(event.target.files?.[0])} /></label>
    {packet && <div className="settings-card">
      <h3>{packet.manifest.name} · {packet.manifest.version}</h3>
      <p>{packet.manifest.id} · {packet.files.length} fichier(s)</p>
      <p>Les empreintes, le contrat et les composants seront vérifiés par le serveur.</p>
      {packet.manifest.bundle?.signature ? <p>Paquet signé : le serveur vérifiera la signature auprès des éditeurs autorisés pour cette instance.</p> : <label><input type="checkbox" checked={approved} onChange={event => setApproved(event.target.checked)} /> J’autorise l’installation de ce paquet non signé dont je connais la provenance.</label>}
      <Button variant="primary" busy={busy} disabled={!approved} onClick={() => void install()}>Vérifier et installer</Button>
    </div>}
    <Button busy={busy} onClick={() => { void refresh().catch(() => setError("Actualisation impossible.")); }}>Actualiser</Button>
    {catalog && <table className="team-table"><caption>Versions installées sur le serveur</caption><thead><tr><th>Lens</th><th>Version</th><th>État</th><th>Projets</th><th>Actions</th></tr></thead><tbody>
      {catalog.items.map(item => <tr key={pinKey(item.pin)}><th>{item.manifest.name}<small> {item.pin.lensId}</small></th><td>{item.pin.version}</td><td>{item.state === "revoked" ? "Révoquée" : item.state === "retired" ? "Retirée du catalogue" : item.available ? "Disponible" : "Indisponible"}</td><td>{item.usageCount ?? "—"}</td><td>
        {item.state === "active" && <Button size="sm" onClick={() => setConfirmation({ title: "Retirer cette version du catalogue", subject: `${item.manifest.name} · ${item.pin.version}`, effects: ["Les projets qui utilisent déjà cette version la conservent. Elle ne sera plus proposée aux nouveaux projets."], confirmLabel: "Retirer du catalogue", run: async () => { await transport({ action: "retireLens", pin: item.pin }); await refresh(); } })}>Retirer</Button>}
        {item.state !== "revoked" && <Button size="sm" onClick={() => setConfirmation({ title: "Révoquer cette version", subject: `${item.manifest.name} · ${item.pin.version}`, effects: ["Le serveur refuse les nouveaux téléchargements et les nouvelles exécutions de cette version.", "Les clients hors ligne ne peuvent apprendre cette révocation qu’à leur reconnexion. Les données historiques sont conservées."], confirmLabel: "Révoquer la version", danger: true, run: async () => { await transport({ action: "revokeLens", pin: item.pin }); await refresh(); } })}>Révoquer</Button>}
      </td></tr>)}
    </tbody></table>}
    <ConfirmDialog value={confirmation} onClose={() => setConfirmation(null)} />
    {Boolean(catalog?.errors.length) && <Banner tone="warning">Certaines lenses ne sont pas disponibles. Vérifiez les diagnostics du serveur.</Banner>}
  </section>;
}

export function ProjectLensesPanel({ transport, scope, onDirtyChange }: { transport: LensTransport; scope: string; onDirtyChange?: (dirty: boolean) => void }) {
  const runtime = useLensRuntime();
  const [value, setValue] = useState<LensConfiguration | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [associations, setAssociations] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState("");
  const [dirty, setDirty] = useState(false);
  function accept(next: LensConfiguration) {
    setValue(next); setSelected(next.pins.map(pinKey));
    setAssociations(next.associations.map(rule => `${rule.extension}=${rule.lensId}`).join("\n"));
    setDirty(false);
  }
  useEffect(() => { let alive = true;
    setValue(null); setError(""); setDirty(false);
    transport<LensConfiguration>({ action: "projectLenses" }).then(next => { if (alive) accept(next); }, () => { if (alive) setError("Les lenses du projet n’ont pas pu être lues."); });
    return () => { alive = false; };
  }, [transport, scope]);
  useEffect(() => { onDirtyChange?.(dirty); return () => onDirtyChange?.(false); }, [dirty, onDirtyChange]);
  async function save() {
    if (!value || busy) return;
    setBusy(true); setError(""); setNotice("");
    try {
      const rules = associations.split("\n").map(line => line.trim()).filter(Boolean).map(line => {
        const parts = line.split("=").map(part => part.trim());
        if (parts.length !== 2 || !parts[0] || !parts[1]) throw Error("Chaque association doit avoir la forme extension=identifiant.lens.");
        return { extension: parts[0].startsWith(".") ? parts[0] : `.${parts[0]}`, lensId: parts[1] };
      });
      const pins = value.availableCatalog.filter(item => selected.includes(pinKey(item.pin))).map(item => item.pin);
      const next = await transport<LensConfiguration>({ action: "configureLenses", request: { expectedRevision: value.revision, pins, associations: rules } });
      accept(next); await runtime?.refresh(); setNotice("Lenses du projet enregistrées.");
    } catch (cause) { setError(cause instanceof Error ? cause.message : "Enregistrement impossible. Actualisez si la configuration a changé."); }
    finally { setBusy(false); }
  }
  return <section className="team-panel" aria-label="Lenses du projet">
    <h2>Lenses du projet</h2>
    <p>Chaque version cochée est demandée aux clients de ce projet. Installer une nouvelle version sur le serveur ne modifie pas cette sélection.</p>
    {error && <Banner tone="danger">{error}</Banner>}{notice && <Banner tone="success">{notice}</Banner>}
    {value && <>
      {!value.canManage && <p>Votre accès permet de consulter cette configuration.</p>}
      {!value.availableCatalog.length && <EmptyState title="Aucune lens disponible">Un administrateur peut installer des lenses dans l’administration de l’instance.</EmptyState>}
      {value.availableCatalog.map(item => <label key={pinKey(item.pin)} style={{ display: "block", marginBlock: 12 }}>
        <input type="checkbox" disabled={busy || !value.canManage || (!item.available && !selected.includes(pinKey(item.pin)))} checked={selected.includes(pinKey(item.pin))} onChange={event => {
          setSelected(previous => event.target.checked ? [...previous.filter(key => !key.startsWith(`${item.pin.lensId}@`)), pinKey(item.pin)] : previous.filter(key => key !== pinKey(item.pin)));
          setDirty(true);
        }} /> {item.manifest.name} · {item.pin.version} · {item.available ? "Disponible sur le serveur" : "Indisponible"}
      </label>)}
      <label>Associations prioritaires (une par ligne : extension=identifiant.lens)
        <textarea rows={4} value={associations} disabled={busy || !value.canManage} onChange={event => { setAssociations(event.target.value); setDirty(true); }} style={{ display: "block", width: "100%" }} />
      </label>
      <p>Sans association explicite, les extensions déclarées par les lenses sont utilisées. Une correspondance ambiguë utilise l’affichage de secours.</p>
      <Button variant="primary" busy={busy} disabled={!dirty || !value.canManage} onClick={() => void save()}>Enregistrer</Button>
      <Button disabled={busy} onClick={() => { void transport<LensConfiguration>({ action: "projectLenses" }).then(accept, () => setError("Actualisation impossible.")); }}>Recharger la configuration</Button>
    </>}
  </section>;
}
