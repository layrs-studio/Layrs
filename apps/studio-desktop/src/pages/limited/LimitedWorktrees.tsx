import { useState } from "react";
import { Banner, Button, Card, Link, PageHeader, TechnicalId, TextField, segment } from "@layrs/ui";
import { openProject } from "../../app/Frame";
import { useConversionInspection } from "../../app/useConversionInspection";
import { useWorkspace } from "../../app/workspace";
import { useOfflineTask } from "../../app/useOfflineTask";
import { conversion, createdFamily, quotaBytes, separateDirectories, type Conversion } from "../../app/offline";
import { AbandonConversionAction } from "./ConversionRecovery";

export function LimitedWorktrees() {
  const inspection = useConversionInspection(), { source, loading, error } = inspection, { root, busy } = useWorkspace();
  return <div className="layrs-page layrs-page--narrow">
    <PageHeader title="Worktrees" description="Identité de cette copie et stockage de ses contenus." />
    <Card className="settings-card"><dl className="settings-facts"><dt>Dossier courant</dt><dd><code>{root}</code></dd><dt>Worktree courant</dt><dd>{source ? <TechnicalId value={source.identity.worktreeId} /> : "À vérifier"}</dd><dt>Stockage</dt><dd>{!source ? "À vérifier" : source.shared ? "Famille partagée" : "Propre à ce dossier"}</dd></dl>
      <p>Cette page inspecte la source en lecture seule. Un format non pris en charge doit être traité séparément ; aucune migration n’est lancée ici.</p>
      <Button busy={loading} busyLabel="Inspection de la source…" disabledReason={busy ? "Opération en cours." : undefined} onClick={() => void inspection.refresh()}>Relire la source avant de reprendre</Button>
      {error && <Banner tone="danger" title="La source ne peut pas être convertie depuis cet état."><code>{error}</code><p><Link to={`/projects/${segment(root)}/health`}>Consulter le diagnostic et les reprises</Link></p></Banner>}
      {source && !source.conversion.allowed && <Banner tone="info" title="Conversion indisponible">{source.conversion.reason}</Banner>}
    </Card>
    {source?.shared === true ? <Card className="settings-card"><p>Cette copie appartient déjà à une famille partagée. La création d’un Worktree supplémentaire depuis cette famille n’est pas encore disponible dans cette interface.</p><Link to={`/projects/${segment(root)}/settings`}>Consulter l’usage et le quota</Link></Card>
      : <ConvertCopy inspection={inspection} />}
  </div>;
}

function ConvertCopy({ inspection }: { inspection: ReturnType<typeof useConversionInspection> }) {
  const { source } = inspection, { root } = useWorkspace(), task = useOfflineTask();
  const [family, setFamily] = useState(""), [destination, setDestination] = useState(""), [quota, setQuota] = useState("");
  const [created, setCreated] = useState<string | null>(null), [receipt, setReceipt] = useState<Conversion | null>(null);
  let familyError: string | undefined, conversionError: string | undefined, resumeError: string | undefined;
  try { separateDirectories(root, family); quotaBytes(quota); } catch (cause) { familyError = String(cause); }
  try { separateDirectories(root, destination, family); } catch (cause) { conversionError = String(cause); }
  try { separateDirectories(root, destination); } catch (cause) { resumeError = String(cause); }
  const busyReason = task.busy ? "Opération en cours." : undefined;
  const blocked = busyReason ?? (!source ? "Inspectez la source avant de poursuivre." : !source.conversion.allowed ? source.conversion.reason ?? "Conversion indisponible." : undefined);
  const execute: typeof task.execute = (label, work) => task.execute(label, async () => {
    try { return await work(); }
    catch (cause) { inspection.invalidate(); throw cause; }
  });
  async function create() {
    if (blocked || familyError || !source) return;
    setCreated(null);
    const result = await execute("Création du stockage partagé…", async () => {
      return createdFamily(await task.local({ action: "createFamily", path: family, quotaBytes: quotaBytes(quota) }), family, source);
    });
    if (result) { setFamily(result); setCreated(result); }
  }
  async function convert(resume: boolean) {
    if (!source || blocked || (resume ? resumeError : conversionError)) return;
    setReceipt(null);
    const result = await execute(resume ? "Reprise de la conversion…" : "Création de la copie partagée…", async () => conversion(await task.local(resume ? { action: "resumeConversion", destination } : { action: "convertIntoFamily", familyRoot: family, destination }), root, destination, source));
    if (result) setReceipt(result);
  }
  return <section className="settings-section" aria-labelledby="convert-copy"><h2 id="convert-copy">Copie avec stockage partagé</h2>
    <Card className="settings-card">
      <p>La conversion crée une copie dans un autre dossier. La source est conservée ; vous choisirez ensuite si vous souhaitez ouvrir la destination.</p>
      <TextField label="Dossier du stockage partagé" hint="Choisissez une famille existante, ou créez-la dans un dossier séparé déjà vide." value={family} disabled={task.busy} onChange={e => { setFamily(e.target.value); setCreated(null); setReceipt(null); }} />
      <details><summary>Créer une nouvelle famille</summary><TextField label="Quota initial (octets)" inputMode="numeric" value={quota} disabled={task.busy} onChange={e => setQuota(e.target.value)} /><Button disabledReason={blocked ?? familyError} onClick={() => void create()}>Créer ce stockage partagé</Button></details>
      {created && <p role="status">Stockage partagé créé : <code>{created}</code></p>}
      <TextField label="Destination de la copie" hint="Dossier existant et vide, distinct de la source et du stockage. Pour une reprise, indiquez la destination interrompue." value={destination} disabled={task.busy} onChange={e => { setDestination(e.target.value); setReceipt(null); }} />
      <div className="sync-card__actions"><Button disabledReason={blocked ?? conversionError} onClick={() => void convert(false)}>Créer la copie en conservant la source</Button><Button disabledReason={blocked ?? resumeError} onClick={() => void convert(true)}>Reprendre cette conversion</Button></div>
      <details><summary>Abandonner une conversion non terminée</summary><AbandonConversionAction family={family} destination={destination} execute={execute} onAbandoned={() => setReceipt(null)} /></details>
      {receipt && <div role="status"><h3>Copie vérifiée</h3><dl className="settings-facts"><dt>Source conservée</dt><dd><code>{receipt.source}</code></dd><dt>Destination</dt><dd><code>{receipt.path}</code></dd><dt>Worktree de destination</dt><dd><TechnicalId value={receipt.worktreeId} /></dd><dt>Captures importées</dt><dd>{receipt.captures}</dd><dt>Empreinte du dossier copié</dt><dd><TechnicalId value={receipt.digest} /></dd></dl><p>L’historique importé est consultable. Une nouvelle capture locale est nécessaire avant toute publication.</p><Button disabledReason={busyReason} onClick={() => openProject(receipt.path)}>Ouvrir la copie</Button></div>}
      {task.error && <Banner tone="danger">{task.error}</Banner>}
    </Card>
  </section>;
}
