import { useState } from "react";
import { Banner, Button, Card, TechnicalId } from "@layrs/ui";
import { useProjection } from "../../app/projection";
import { useRemote } from "../../app/remote";
import { useOfflineTask } from "../../app/useOfflineTask";
import * as offline from "../../app/offline";

export function OfflinePreparations({ networkBlocked }: { networkBlocked?: string }) {
  const { data } = useProjection(), remote = useRemote(), task = useOfflineTask();
  const [items, setItems] = useState<offline.Preparation[] | null>(null);
  const [notice, setNotice] = useState("");
  const [page, setPage] = useState(0);
  async function load() {
    setItems(null); setNotice("");
    const next = await task.execute("Lecture des préparations…", async () => offline.preparations(await task.local({ action: "preparations" })));
    if (next) { setItems(next); setPage(0); }
  }
  async function change(p: offline.Preparation, release: boolean) {
    if (!data || task.blocked || p.status === "released" || !release && networkBlocked) return;
    setNotice("");
    const next = await task.execute(release ? "Libération de la préparation…" : "Reprise de la préparation…", async () => {
      if (release) {
        const result = await task.local<{ released: string }>({ action: "releasePreparation", id: p.id });
        if (result.released !== p.id) throw new Error("La préparation libérée ne correspond pas à la demande.");
      } else {
        offline.preparedReply(await remote.transport({ action: "resumeProjectionOffline", ...offline.networkRequest(data, data.projection.generation), preparationId: p.id }), p.request, p.id);
      }
      return offline.preparations(await task.local({ action: "preparations" }));
    });
    if (next) { setItems(next); setNotice(release ? p.status === "acquiring" ? "Acquisition annulée et réservation libérée." : "Protection libérée. Les octets ne sont pas effacés par cette action." : "Reprise terminée ; état des préparations actualisé."); }
  }
  return <Card className="settings-card">
    <h3>Préparations conservées</h3>
    <p>Libérer une préparation retire sa protection ; annuler une acquisition interrompue libère aussi sa réservation. Les copies et autres préparations peuvent encore retenir ces contenus. L’éviction se demande séparément dans les réglages.</p>
    <Button disabledReason={task.blocked} onClick={() => void load()}>Actualiser les préparations</Button>
    {items && (items.length ? <ul>{items.slice(page * 25, (page + 1) * 25).map(p => <li key={p.id}>
      <TechnicalId value={p.id} /> · {p.status === "released" ? "Libérée ou annulée" : p.status === "ready" ? "Prête" : "Acquisition interrompue ou en cours"}
      <details><summary>{offline.versionLabel(p.request.version)} · {p.request.paths.length} chemin(s)</summary>
        <p>Opération : {{ read: "Lire", edit: "Modifier", compareMetadata: "Comparer les métadonnées", compareContent: "Comparer les contenus" }[p.request.operation]}</p>
        {[p.request.version, ...(p.request.other ? [p.request.other] : [])].map((v, i) => <p key={i}>{offline.versionLabel(v)} · <TechnicalId value={v.kind === "remote" ? v.observationDigest : v.digest} /></p>)}
        <ul>{p.request.paths.map(path => <li key={path}><code>{path}</code></li>)}</ul>
      </details>
      {p.status !== "released" && <div className="sync-card__actions"><Button disabledReason={task.blocked ?? networkBlocked} onClick={() => void change(p, false)}>Reprendre cette préparation</Button><Button disabledReason={task.blocked} onClick={() => void change(p, true)}>{p.status === "acquiring" ? "Annuler cette préparation" : "Libérer cette préparation"}</Button></div>}
    </li>)}</ul> : <p>Aucune préparation enregistrée.</p>)}
    {items && items.length > 25 && <div className="sync-card__actions"><Button disabled={page === 0} onClick={() => setPage(p => p - 1)}>Préparations précédentes</Button><span>Page {page + 1} sur {Math.ceil(items.length / 25)}</span><Button disabled={(page + 1) * 25 >= items.length} onClick={() => setPage(p => p + 1)}>Préparations suivantes</Button></div>}
    {notice && <p role="status">{notice}</p>}{task.error && <Banner tone="danger">{task.error}</Banner>}
  </Card>;
}
