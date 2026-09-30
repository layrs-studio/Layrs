import { useState } from "react";
import { CloudUpload, FolderOpen, LogOut, Plug, RefreshCw, ServerCog, Shuffle } from "lucide-react";
import { Button, Card, Dialog, PageHeader, SelectField, TextField, navigate, useToast } from "@layrs/ui";
import { useProjection } from "../app/projection";
import { useRemote } from "../app/remote";
import { rememberProject } from "../app/recents";
import { rememberOrigin, rememberTarget } from "../app/serverMemory";
import { useWorkspace } from "../app/workspace";
import { selectFolder } from "../tauri";
import { ChooseHeads } from "./ChooseHeads";
import { ActivityList, NoticeView, PendingState, ServerSummary, SyncActions, TransferView, connectPath } from "./parts";

/** P11 detail route: server, transfers, activity after restart and rare server operations. */
export function SyncPage() {
  const workspace = useWorkspace();
  const remote = useRemote();
  const connected = remote.session.state === "connected";
  const projection = workspace.kind === "projection";
  return (
    <div className="layrs-page layrs-page--narrow">
      <PageHeader
        title="Synchronisation"
        description="Publier envoie vos captures et attend leur acceptation par le serveur ; recevoir récupère l’état autorisé de l’équipe. Les deux restent distincts."
      />
      <section className="settings-section" aria-labelledby="sync-server">
        <h2 id="sync-server">Serveur et compte</h2>
        <Card className="sync-card">
          <ServerSummary />
          <div className="sync-card__actions">
            <Button size="sm" icon={<RefreshCw aria-hidden size={14} />} disabledReason={!remote.origin ? "Aucun serveur choisi." : undefined} onClick={() => void remote.check()}>
              Vérifier la session
            </Button>
            <Button size="sm" icon={<Plug aria-hidden size={14} />} onClick={() => navigate(connectPath(workspace.root), { query: { from: window.location.hash.slice(1) } })}>
              {remote.fixed ? "Connexion…" : "Changer de serveur…"}
            </Button>
            {connected && (
              <Button size="sm" variant="ghost" icon={<LogOut aria-hidden size={14} />} onClick={() => void remote.logout()}>
                Se déconnecter
              </Button>
            )}
          </div>
        </Card>
      </section>

      <section className="settings-section" aria-labelledby="sync-actions">
        <h2 id="sync-actions">Publier et recevoir</h2>
        <Card className="sync-card">
          {projection && <TargetLayer />}
          <PendingState />
          <SyncActions />
          {remote.transfer && <TransferView transfer={remote.transfer} />}
          <NoticeView notice={remote.notice} onClose={remote.clearNotice} />
        </Card>
      </section>

      <section className="settings-section" aria-labelledby="sync-activity">
        <h2 id="sync-activity">Activité de cet ordinateur</h2>
        <p className="settings-section__hint">
          Transferts de ce projet observés sur ce poste, conservés après redémarrage. Le moteur ne communique pas encore la progression en octets : aucune
          estimation n’est affichée.
        </p>
        <Card className="sync-card">
          <ActivityList entries={remote.activity} />
        </Card>
      </section>

      {!projection && connected && <AdvancedServerOperations />}
    </div>
  );
}

function TargetLayer() {
  const projection = useProjection();
  const [value, setValue] = useState(projection.target?.layerId ?? "");
  const locked = Boolean(projection.data?.pendingPublication);
  return (
    <div className="sync-target-form">
      <TextField
        label="Layer distant cible"
        value={locked ? projection.data?.pendingPublication?.layerId ?? value : value}
        readOnly={locked}
        placeholder="layer:…"
        hint={
          locked
            ? "Une publication attend son reçu : sa cible enregistrée est conservée."
            : "Identifiant du Layer publié qui reçoit vos captures. Il est mémorisé pour ce dossier."
        }
        onChange={(event) => setValue(event.target.value)}
        onBlur={() => !locked && projection.setTarget(value.trim() ? { layerId: value.trim(), name: projection.target?.layerId === value.trim() ? projection.target.name : undefined } : null)}
      />
    </div>
  );
}

function AdvancedServerOperations() {
  const workspace = useWorkspace();
  const remote = useRemote();
  const toast = useToast();
  const layers = workspace.data?.state.layers ?? [];
  const [organisation, setOrganisation] = useState("");
  const [slug, setSlug] = useState("");
  const [migrating, setMigrating] = useState(false);
  const [destination, setDestination] = useState("");
  const [layer, setLayer] = useState(workspace.activeLayer?.layerId ?? layers[0]?.layerId ?? "");
  const locked = workspace.busy ? `Opération en cours : ${workspace.busy}` : remote.transfer ? "Un transfert est en cours." : undefined;

  async function migrate() {
    const target = destination.trim();
    const chosen = layers.find((item) => item.layerId === layer);
    // Non-secret preferences of the new folder, written before it opens.
    rememberTarget(target, { layerId: layer, name: chosen?.name });
    rememberOrigin(target, remote.origin);
    rememberProject({ root: target, name: "", kind: "projection" });
    const result = await remote.operate(
      { kind: "migrate", label: "Création d’un accès limité", busy: "Création de l’accès limité…", context: "clone" },
      { action: "migrateProjection", destination: target, layerId: layer },
    );
    if (result) {
      setMigrating(false);
      toast.show({ tone: "success", title: "Accès limité créé", description: "Le dossier d’origine et son historique sont conservés." });
    }
  }

  return (
    <section className="settings-section" aria-labelledby="sync-advanced">
      <h2 id="sync-advanced">Avancé</h2>
      <p className="settings-section__hint">Opérations rares sur le serveur. Chacune indique ce qu’elle change et ce qui est conservé.</p>
      <Card className="sync-card sync-advanced">
        <details>
          <summary>
            <CloudUpload aria-hidden size={16} /> Publier en conservant les têtes concurrentes
          </summary>
          <p>
            Publie vos captures sans remplacer les têtes déjà présentes sur le serveur pour ce Layer. Une fusion restera nécessaire pour les réunir.
          </p>
          <Button
            disabledReason={locked}
            onClick={() =>
              void remote.operate(
                { kind: "publishIntegrate", label: "Publication en conservant les têtes concurrentes", busy: "Publication…", context: "publish", cancellable: true },
                { action: "publish", integrate: true },
              )
            }
          >
            Publier en conservant les têtes concurrentes
          </Button>
        </details>
        <details>
          <summary>
            <ServerCog aria-hidden size={16} /> Créer ce projet sur le serveur
          </summary>
          <p>Crée le projet dans une organisation du serveur et y relie ce dossier. Vos fichiers locaux ne sont pas modifiés.</p>
          <div className="sync-advanced__grid">
            <TextField label="Organisation" value={organisation} onChange={(event) => setOrganisation(event.target.value)} />
            <TextField label="Identifiant du projet" value={slug} hint="Lettres minuscules, chiffres et tirets." onChange={(event) => setSlug(event.target.value)} />
          </div>
          <Button
            disabledReason={locked ?? (!organisation.trim() || !slug.trim() ? "Indiquez l’organisation et l’identifiant." : undefined)}
            onClick={() =>
              void remote.operate(
                { kind: "provision", label: "Création du projet sur le serveur", busy: "Création sur le serveur…", context: "remote" },
                { action: "provision", workspace: organisation.trim(), slug: slug.trim() },
              )
            }
          >
            Créer le projet sur le serveur
          </Button>
        </details>
        <ChooseHeads locked={locked} />
        <details>
          <summary>
            <Shuffle aria-hidden size={16} /> Passer en accès limité
          </summary>
          <p>
            Crée dans un autre dossier vide une copie limitée aux fichiers que votre compte peut lire. Le projet doit être publié et sans modification locale. Ce
            dossier et son historique sont conservés : aucune migration silencieuse.
          </p>
          <Button disabledReason={locked} onClick={() => setMigrating(true)}>
            Préparer l’accès limité…
          </Button>
        </details>
      </Card>
      <Dialog
        open={migrating}
        onClose={() => setMigrating(false)}
        title="Passer en accès limité"
        description="Les fichiers autorisés sont copiés dans le dossier choisi ; les chemins interdits restent visibles comme réservés, sans contenu."
        dismissible={!workspace.busy}
        footer={
          <>
            <Button onClick={() => setMigrating(false)} disabled={Boolean(workspace.busy)}>
              Annuler
            </Button>
            <Button
              variant="primary"
              busy={workspace.busy === "Création de l’accès limité…"}
              busyLabel="Création…"
              disabledReason={!destination.trim() ? "Choisissez un dossier vide." : !layer ? "Choisissez le Layer publié." : undefined}
              onClick={() => void migrate()}
            >
              Créer l’accès limité et conserver ce dossier
            </Button>
          </>
        }
      >
        <div className="new-project__folder">
          <TextField label="Dossier vide de destination" value={destination} onChange={(event) => setDestination(event.target.value)} />
          <Button
            icon={<FolderOpen aria-hidden size={16} />}
            onClick={() => void selectFolder(destination).then((folder) => folder && setDestination(folder), () => undefined)}
          >
            Parcourir…
          </Button>
        </div>
        <SelectField label="Layer publié à projeter" value={layer} onChange={(event) => setLayer(event.target.value)}>
          {layers.map((item) => (
            <option key={item.layerId} value={item.layerId}>
              {item.name}
            </option>
          ))}
        </SelectField>
        <NoticeView notice={remote.notice} onClose={remote.clearNotice} />
      </Dialog>
    </section>
  );
}
