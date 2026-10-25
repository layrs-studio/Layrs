import { useCallback, useEffect, useRef, useState } from "react";
import { FolderOpen, FolderPlus, RefreshCw } from "lucide-react";
import { Badge, Banner, Button, Card, Link, Loading, PageHeader, SelectField, TechnicalId, TextField, navigate, segment } from "@layrs/ui";
import { nativeWorkspace, selectFolder } from "../tauri";
import { useWorkspace } from "../app/workspace";
import { createWorktreeRequest, currentWorktree, worktreeDestination, type WorktreeList, type WorktreeReply } from "../app/worktrees";

export function WorktreesPage() {
  const workspace = useWorkspace();
  const { root, data, busy, task } = workspace;
  const [list, setList] = useState<WorktreeList | null>(null);
  const [path, setPath] = useState("");
  const [layer, setLayer] = useState("");
  const [step, setStep] = useState("");
  const [failedPath, setFailedPath] = useState("");
  const revision = useRef(0);
  const initialRead = useRef(false);
  const own = data && currentWorktree(data.state);

  const refresh = useCallback(async () => {
    const current = ++revision.current;
    const result = await task("Lecture des Worktrees…", () => nativeWorkspace<WorktreeList>(root, { action: "listWorktrees" }));
    if (current === revision.current && result) setList(result);
  }, [root, task]);

  useEffect(() => {
    if (busy || initialRead.current) return;
    let disposed = false;
    // Wait for the opening lock and StrictMode's setup/cleanup cycle before reading.
    void Promise.resolve().then(() => {
      if (!disposed) { initialRead.current = true; void refresh(); }
    });
    return () => { disposed = true; };
  }, [busy, refresh]);
  useEffect(() => () => { revision.current++; }, []);

  function go(reply: WorktreeReply) {
    const destination = worktreeDestination(reply);
    navigate(`/projects/${segment(destination.path)}/${destination.view}`);
  }

  async function open(path: string) {
    const current = revision.current;
    await task("Ouverture du Worktree…", async () => {
      const reply = await nativeWorkspace<WorktreeReply>(root, { action: "openWorktree", path });
      if (current === revision.current) go(reply);
    });
  }

  async function browse(create: boolean) {
    const current = revision.current;
    const selected = await task("Choix du dossier…", () => selectFolder(create ? path : undefined));
    if (!selected || current !== revision.current) return;
    if (create) setPath(selected);
    else await open(selected);
  }

  async function resumeFailedCreation() {
    if (!failedPath || busy) return;
    const current = revision.current;
    await task("Reprise de la création…", async () => {
      // The target may only have a pending marker, so do not require openWorktree first.
      const reply = await nativeWorkspace<WorktreeReply>(failedPath, { action: "resumeWorktreeCreation" });
      if (current === revision.current) go(reply);
    });
  }

  let request: ReturnType<typeof createWorktreeRequest> | undefined;
  let invalid: string | undefined;
  try { request = createWorktreeRequest(path, layer, step); }
  catch (cause) { invalid = cause instanceof Error ? cause.message : String(cause); }
  const blocked = busy ? `Opération en cours : ${busy}` : !data ? "Actualisez l’état du projet." : data.needsRepair || own?.status === "stale" || own?.status === "needs_repair" ? "Terminez la reprise de ce Worktree." : invalid;

  async function create() {
    if (blocked || !request) return;
    const selected = request;
    const current = revision.current;
    setFailedPath("");
    const result = await task("Création du Worktree…", async () => {
      const reply = await nativeWorkspace<WorktreeReply>(root, selected);
      worktreeDestination(reply);
      return reply;
    });
    if (current !== revision.current) return;
    if (result) go(result);
    else setFailedPath(selected.path);
  }

  return (
    <div className="layrs-page layrs-page--narrow">
      <PageHeader title="Worktrees" description="Dossiers de travail indépendants qui partagent ce Store. Ouvrir un autre dossier change le projet affiché dans cette fenêtre." aside={
        <Button icon={<FolderOpen aria-hidden size={16} />} disabled={Boolean(busy)} onClick={() => void browse(false)}>Ouvrir un Worktree…</Button>
      } />
      <section className="settings-section" aria-labelledby="current-worktree">
        <h2 id="current-worktree">Worktree courant</h2>
        <Card className="settings-card">
          <dl className="settings-facts">
            <dt>Dossier</dt><dd><code>{data?.state.worktree.checkoutRoot ?? root}</code></dd>
            <dt>Worktree</dt><dd>{data ? <TechnicalId value={data.state.worktree.worktreeId} /> : "—"}</dd>
            <dt>Layer</dt><dd>{workspace.activeLayer?.name ?? "—"}</dd>
            <dt>Step courante</dt><dd>{own?.baseStepId ? <TechnicalId value={own.baseStepId} /> : "Non disponible"}</dd>
            <dt>Opération de ce travail</dt><dd>{data ? <TechnicalId value={data.state.operation.operationId} /> : "—"}</dd>
          </dl>
          {(own?.status === "stale" || own?.status === "needs_repair" || data?.needsRepair) && <Link to={`/projects/${segment(root)}/health`}>Diagnostic et reprise de ce Worktree</Link>}
        </Card>
      </section>
      <section className="settings-section" aria-labelledby="registered-worktrees">
        <h2 id="registered-worktrees">Dossiers enregistrés</h2>
        <Card className="settings-card">
          <div><Button size="sm" icon={<RefreshCw aria-hidden size={14} />} disabled={Boolean(busy)} onClick={() => void refresh()}>Actualiser les Worktrees</Button></div>
          {!list ? busy === "Lecture des Worktrees…" ? <Loading label="Lecture des Worktrees…" /> : <p>La liste n’a pas pu être chargée. Actualisez pour réessayer.</p> : <ul className="layer-list">
            {list.worktrees.map(item => (
              <li key={item.worktreeId} className="layer-list__item">
                <div className="layer-list__text">
                  <p className="layer-list__name"><code>{item.checkoutRoot}</code></p>
                  <p className="layer-list__meta"><TechnicalId value={item.worktreeId} /></p>
                </div>
                {item.worktreeId === list.currentWorktreeId ? <Badge tone="accent">Courant</Badge> : <Button size="sm" disabled={Boolean(busy)} onClick={() => void open(item.checkoutRoot)} aria-label={`Ouvrir ${item.checkoutRoot}`}>Ouvrir</Button>}
              </li>
            ))}
          </ul>}
          <p className="sync-muted">Chaque dossier conserve sa Step, son opération et ses fichiers modifiables. Les états des autres Worktrees sont relus à leur ouverture.</p>
        </Card>
      </section>
      <section className="settings-section" aria-labelledby="new-worktree">
        <h2 id="new-worktree">Créer un Worktree secondaire</h2>
        <Card className="settings-card">
          <div className="new-project__folder">
            <TextField label="Dossier du nouveau Worktree" value={path} disabled={Boolean(busy)} hint="Un dossier existant, vide et distinct des autres Worktrees." onChange={event => setPath(event.target.value)} />
            <Button disabled={Boolean(busy)} onClick={() => void browse(true)}>Parcourir…</Button>
          </div>
          <SelectField label="Layer du nouveau Worktree" value={layer} disabled={Boolean(busy)} onChange={event => setLayer(event.target.value)}>
            <option value="">Choisir un Layer existant</option>
            {data?.state.layers.map(item => <option key={item.layerId} value={item.layerId}>{item.name}</option>)}
          </SelectField>
          <TextField label="Step exacte de départ" value={step} disabled={Boolean(busy)} placeholder="blake3:…" hint="Cette base reste fixe pendant la création, même si un autre Worktree avance." onChange={event => setStep(event.target.value)} />
          {own?.baseStepId && <div><Button size="sm" disabled={Boolean(busy)} onClick={() => setStep(own.baseStepId!)}>Utiliser la Step courante de ce Worktree</Button></div>}
          <p className="sync-muted">Le Layer choisi doit déjà exister. Créer ce dossier ne change pas la Step des autres Worktrees.</p>
          <div><Button variant="primary" icon={<FolderPlus aria-hidden size={16} />} busy={busy === "Création du Worktree…"} busyLabel="Création…" disabledReason={blocked} onClick={() => void create()}>Créer et ouvrir le Worktree</Button></div>
          {failedPath && <Banner tone="warning" title="Le Worktree n’a pas été confirmé prêt" actions={
            <><Button disabled={Boolean(busy)} onClick={() => void open(failedPath)}>Ouvrir le dossier pour reprendre</Button>
              <Button disabled={Boolean(busy)} onClick={() => void resumeFailedCreation()}>Reprendre la création dans ce dossier</Button></>
          }>Le dossier <code>{failedPath}</code> peut contenir une création interrompue, même avant son enregistrement. Reprenez-la directement ou ouvrez son diagnostic. Les collisions et erreurs restent à résoudre ; aucun dossier n’est effacé.</Banner>}
        </Card>
      </section>
    </div>
  );
}
