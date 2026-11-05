import { useRef, useState } from "react";
import { Banner, Button, Card, Link, TextField, formatBytes, segment } from "@layrs/ui";
import { useProjection } from "../../app/projection";
import { useWorkspace } from "../../app/workspace";
import { useOfflineTask } from "../../app/useOfflineTask";
import { quotaBytes, releaseGenerationRequest, releasedGeneration, usage, type Usage } from "../../app/offline";

export function SharedStorageSettings() {
  const { data } = useProjection(), { root } = useWorkspace();
  // Retain form input through a refused native action. A missing current status
  // still disables writes until explicitly refreshed; it grants no capability.
  const wasShared = useRef(false);
  if (data) wasShared.current = data.offline?.shared === true;
  return <section className="settings-section" aria-labelledby="shared-storage"><h2 id="shared-storage">Stockage partagé et quota</h2>
    {wasShared.current ? <FamilyUsage />
      : <Card className="settings-card"><p>Ce dossier utilise son stockage propre. Les quotas partagés et la conservation hors ligne sont disponibles dans une copie avec stockage partagé.</p><Link to={`/projects/${segment(root)}/worktrees`}>Préparer une copie séparée</Link></Card>}
  </section>;
}
function FamilyUsage() {
  const task = useOfflineTask();
  const projection = useProjection(), { data } = projection;
  const [current, setCurrent] = useState<Usage | null>(null), [quota, setQuota] = useState("");
  const [notice, setNotice] = useState("");
  const [generation, setGeneration] = useState("");
  const [releaseNotice, setReleaseNotice] = useState("");
  let releaseRequest: ReturnType<typeof releaseGenerationRequest> | undefined, generationError: string | undefined;
  try { if (data) releaseRequest = releaseGenerationRequest(generation, data); }
  catch (cause) { generationError = cause instanceof Error ? cause.message : String(cause); }
  let bytes: number | undefined, error: string | undefined;
  try { bytes = quotaBytes(quota); } catch (cause) { error = cause instanceof Error ? cause.message : String(cause); }
  async function run(action: "familyUsage" | "setFamilyQuota" | "evictUnretained") {
    if (task.blocked || action === "setFamilyQuota" && error) return;
    setNotice(""); setCurrent(null);
    const result = await task.execute("Gestion du stockage partagé…", async () => {
      const reply = await task.local<{ usage: Usage; evictedObjects?: number }>({ action, ...(action === "setFamilyQuota" ? { bytes } : {}) });
      const next = usage(reply);
      if (action === "evictUnretained" && (!Number.isSafeInteger(reply.evictedObjects) || reply.evictedObjects! < 0)) throw new Error("Résultat d’éviction invalide.");
      return { next, count: reply.evictedObjects };
    });
    if (result) {
      setCurrent(result.next);
      if (action === "familyUsage") setQuota(String(result.next.limitBytes));
      setNotice(action === "evictUnretained" ? `${result.count} objet(s) non retenu(s) évincé(s).` : action === "setFamilyQuota" ? "Quota partagé enregistré." : "Usage du stockage actualisé.");
    }
  }
  async function releaseGeneration() {
    if (task.blocked || !releaseRequest) return;
    setReleaseNotice(""); setCurrent(null); setNotice("");
    const count = await task.execute("Libération de la protection de génération…", async () => releasedGeneration(await task.local(releaseRequest), releaseRequest.generation));
    if (count !== undefined) setReleaseNotice(`Génération ${releaseRequest.generation} : ${count} protection(s) libérée(s). Aucun contenu n’a été évincé par cette action.`);
  }
  return <Card className="settings-card">
    <p>Le quota et l’éviction concernent toutes les copies de cette famille. Les contenus retenus par une copie ou une préparation restent protégés.</p>
    <Button disabledReason={task.blocked} onClick={() => void run("familyUsage")}>Actualiser l’usage partagé</Button>
    {!data && <Button disabledReason={task.busy ? "Opération en cours." : undefined} onClick={() => void projection.refresh()}>Relire l’état du dossier</Button>}
    {current && <dl className="settings-facts"><dt>Contenus locaux</dt><dd>{formatBytes(current.contentBytes)}</dd><dt>Réservations en cours</dt><dd>{formatBytes(current.reservedBytes)}</dd><dt>Quota</dt><dd>{formatBytes(current.limitBytes)} · {current.limitBytes} octets</dd><dt>Racines de protection</dt><dd>{current.protectionRoots}</dd></dl>}
    <TextField label="Quota partagé (octets)" inputMode="numeric" value={quota} error={quota ? error : undefined} disabled={task.busy} onChange={e => setQuota(e.target.value)} />
    <div className="sync-card__actions"><Button disabledReason={task.blocked ?? error} onClick={() => void run("setFamilyQuota")}>Appliquer ce quota</Button><Button disabledReason={task.blocked ?? (!current ? "Actualisez l’usage avant de demander l’éviction." : undefined)} onClick={() => void run("evictUnretained")}>Évincer les contenus non retenus</Button></div>
    <p className="sync-muted">L’éviction ne réduit pas la sélection du dossier. Un contenu évincé peut nécessiter un téléchargement ultérieur.</p>
    <details><summary>Libérer la protection d’une ancienne génération</summary>
      <p>Indiquez le numéro exact de la génération concernée. La base courante, l’historique des captures et les préparations restent protégés : le moteur refusera toute dépendance. Cette action n’efface ni l’historique ni les fichiers ; l’éviction reste une action séparée.</p>
      {data && <p>Génération courante protégée : {data.projection.generation}.</p>}
      <TextField label="Génération exacte à libérer" inputMode="numeric" value={generation} error={generation ? generationError : undefined} disabled={task.busy} onChange={e => { setGeneration(e.target.value); setReleaseNotice(""); }} />
      <Button disabledReason={task.blocked ?? generationError ?? (!releaseRequest ? "Indiquez la génération à libérer." : undefined)} onClick={() => void releaseGeneration()}>Libérer la protection de cette génération</Button>
      {releaseNotice && <p role="status">{releaseNotice}</p>}
    </details>
    {notice && <p role="status">{notice}</p>}{task.error && <Banner tone="danger">{task.error}</Banner>}
  </Card>;
}
