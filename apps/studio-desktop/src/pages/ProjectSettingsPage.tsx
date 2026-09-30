import { useEffect, useRef, useState } from "react";
import { Button, Card, Link, PageHeader, SelectField, Switch, TechnicalId, TextField, navigate, segment, useToast } from "@layrs/ui";
import { useProjection } from "../app/projection";
import { useRemote } from "../app/remote";
import { forgetOrigin, hostOf } from "../app/serverMemory";
import { useWorkspace } from "../app/workspace";
import { nativeWorkspace } from "../tauri";

/** P17 on Desktop: this folder, local capture, identity of future captures, server and advanced tools. */
export function ProjectSettingsPage() {
  const workspace = useWorkspace();
  const projection = workspace.kind === "projection";
  return (
    <div className="layrs-page layrs-page--narrow">
      <PageHeader title="Réglages du projet" description="Préférences de ce dossier sur cet ordinateur. Elles ne modifient pas les règles de l’équipe." />
      {projection ? <LimitedGeneral /> : <General />}
      {!projection && <LocalCapture />}
      {!projection && <IdentitySettings />}
      <ServerSettings />
      {!projection && <Rewrite />}
    </div>
  );
}

function General() {
  const workspace = useWorkspace();
  const { data } = workspace;
  return (
    <section className="settings-section" aria-labelledby="settings-general">
      <h2 id="settings-general">Général</h2>
      <Card className="settings-card">
        <dl className="settings-facts">
          <dt>Nom</dt>
          <dd>{data?.state.space.name ?? "—"}</dd>
          <dt>Dossier</dt>
          <dd>
            <code>{workspace.root}</code>
          </dd>
          <dt>Type</dt>
          <dd>Projet complet sur cet ordinateur</dd>
          <dt>Référence technique</dt>
          <dd>{data ? <TechnicalId value={data.state.space.spaceId} /> : "—"}</dd>
        </dl>
      </Card>
    </section>
  );
}

function LimitedGeneral() {
  const workspace = useWorkspace();
  const { data } = useProjection();
  return (
    <section className="settings-section" aria-labelledby="settings-general">
      <h2 id="settings-general">Général</h2>
      <Card className="settings-card">
        <dl className="settings-facts">
          <dt>Dossier</dt>
          <dd>
            <code>{workspace.root}</code>
          </dd>
          <dt>Type</dt>
          <dd>Accès limité : seuls les fichiers autorisés sont présents</dd>
          <dt>Serveur</dt>
          <dd>{data ? hostOf(data.identity.origin) : "—"}</dd>
          <dt>Compte</dt>
          <dd>{data ? <TechnicalId value={data.identity.principalId} /> : "—"}</dd>
          <dt>Référence technique</dt>
          <dd>{data ? <TechnicalId value={data.identity.spaceId} /> : "—"}</dd>
        </dl>
      </Card>
    </section>
  );
}

function LocalCapture() {
  const workspace = useWorkspace();
  const settings = workspace.activeLayer?.settings;
  return (
    <section className="settings-section" aria-labelledby="settings-capture">
      <h2 id="settings-capture">Capture locale</h2>
      <p className="settings-section__hint">
        Après une période sans modification, Layrs capture le travail de ce dossier tant que le projet est ouvert. Rien n’est publié.
      </p>
      <Card className="settings-card">
        <Switch
          label="Capture automatique pendant cette session"
          hint={
            settings?.automaticCapture === false
              ? "Désactivée par le réglage du Layer courant."
              : `Délai d’inactivité du Layer ${workspace.activeLayer?.name ?? ""} : ${Math.round((settings?.automaticCaptureIdleMs ?? 0) / 1000)} s. La pause dure jusqu’à la fermeture du projet.`
          }
          checked={workspace.automatic}
          onChange={workspace.setAutomatic}
        />
      </Card>
    </section>
  );
}

type LocalIdentity = { actorId: string; deviceId: string | null; displayName: string | null };

/** Declared identity of future captures; old observations and server rights are unchanged. */
function IdentitySettings() {
  const workspace = useWorkspace();
  const toast = useToast();
  const [identity, setIdentity] = useState<LocalIdentity | null>(null);
  const [name, setName] = useState("");
  const [ready, setReady] = useState(false);
  const [error, setError] = useState("");
  const generation = useRef(0);

  useEffect(() => {
    const current = ++generation.current;
    setReady(false);
    nativeWorkspace<{ identity: LocalIdentity | null }>(workspace.root, { action: "localIdentity" })
      .then((result) => {
        if (generation.current !== current) return;
        setIdentity(result.identity);
        setName(result.identity?.displayName || "");
        setReady(true);
      })
      .catch((cause) => generation.current === current && setError(String(cause)));
  }, [workspace.root]);

  async function save() {
    const next: LocalIdentity = {
      actorId: identity?.actorId || `actor:${crypto.randomUUID()}`,
      deviceId: identity?.deviceId || `device:${crypto.randomUUID()}`,
      displayName: name.trim(),
    };
    const result = await workspace.task("Enregistrement de l’identité…", () => nativeWorkspace<{ identity: LocalIdentity }>(workspace.root, { action: "configureIdentity", identity: next }));
    if (result) {
      setIdentity(result.identity);
      toast.show({ tone: "success", title: "Identité enregistrée", description: "Elle attribue vos prochaines captures sur cet ordinateur." });
    }
  }

  return (
    <section className="settings-section" aria-labelledby="settings-identity">
      <h2 id="settings-identity">Identité des prochaines captures</h2>
      <p className="settings-section__hint">
        Nom déclaré pour les captures futures de ce poste. Il ne réécrit pas les anciennes captures et n’accorde aucun droit sur le serveur ; ce n’est pas
        votre profil de connexion.
      </p>
      <Card className="settings-card">
        <form
          className="settings-form"
          onSubmit={(event) => {
            event.preventDefault();
            void save();
          }}
        >
          <TextField label="Nom affiché" value={name} maxLength={256} disabled={!ready} error={error || undefined} onChange={(event) => setName(event.target.value)} />
          <div>
            <Button
              type="submit"
              busy={workspace.busy === "Enregistrement de l’identité…"}
              disabledReason={!ready ? "Lecture de l’identité…" : !name.trim() ? "Indiquez un nom." : workspace.busy ? "Opération en cours." : undefined}
            >
              Enregistrer l’identité locale
            </Button>
          </div>
        </form>
        {identity && (
          <dl className="settings-facts">
            <dt>Identifiant pour les filtres d’historique</dt>
            <dd>
              <TechnicalId value={identity.actorId} />
            </dd>
          </dl>
        )}
      </Card>
    </section>
  );
}

function ServerSettings() {
  const workspace = useWorkspace();
  const remote = useRemote();
  const [forgotten, setForgotten] = useState(false);
  return (
    <section className="settings-section" aria-labelledby="settings-server">
      <h2 id="settings-server">Serveur</h2>
      <p className="settings-section__hint">
        Le serveur est mémorisé pour ce dossier sur cet ordinateur. Le jeton de connexion reste dans le coffre du système.
      </p>
      <Card className="settings-card">
        <dl className="settings-facts">
          <dt>Adresse</dt>
          <dd>{remote.origin ? <code>{remote.origin}</code> : "Aucun serveur"}</dd>
          <dt>Certificat d’entreprise</dt>
          <dd>{remote.authority ? "Autorité privée mémorisée pour ce serveur" : "Certificats reconnus par le système"}</dd>
        </dl>
        <div className="sync-card__actions">
          <Button size="sm" onClick={() => navigate(`/projects/${segment(workspace.root)}/connect`)}>
            Gérer la connexion…
          </Button>
          {!remote.fixed && remote.origin && (
            <Button
              size="sm"
              variant="ghost"
              disabledReason={forgotten ? "Ce dossier n’a plus de serveur mémorisé." : undefined}
              onClick={() => {
                forgetOrigin(workspace.root);
                setForgotten(true);
              }}
            >
              Oublier ce serveur pour ce dossier
            </Button>
          )}
        </div>
      </Card>
    </section>
  );
}

function Rewrite() {
  const workspace = useWorkspace();
  const { data, busy } = workspace;
  const locked = Boolean(busy) || Boolean(data?.needsRepair);
  const [kind, setKind] = useState("replay");
  const [source, setSource] = useState("");
  const [mainline, setMainline] = useState("");
  const listId = "rewrite-references";

  function preview() {
    const params = { kind, source: source.trim(), mainline: kind === "squash" ? null : mainline.trim() || null };
    void workspace.inspect({ action: "previewRewrite", ...params }, { action: "applyRewrite", ...params }, "Transformer des changements");
  }

  return (
    <section className="settings-section" aria-labelledby="settings-advanced">
      <h2 id="settings-advanced">Avancé</h2>
      <p className="settings-section__hint">
        Transformer des changements : rejouer, inverser ou regrouper des captures. Un aperçu montre l’impact sur les Layers et les points nommés avant toute
        écriture. Pour passer en accès limité, voir <Link to={`/projects/${segment(workspace.root)}/sync`}>Synchronisation › Avancé</Link>.
      </p>
      <Card className="settings-card">
        <SelectField label="Action" value={kind} onChange={(event) => setKind(event.target.value)}>
          <option value="replay">Rejouer un changement</option>
          <option value="revert">Inverser un changement</option>
          <option value="squash">Regrouper jusqu’à la capture courante</option>
        </SelectField>
        <TextField label="Capture ou référence" value={source} list={listId} placeholder="blake3:… ou @marker:nom" onChange={(event) => setSource(event.target.value)} />
        <datalist id={listId}>
          {data?.markers.map((marker) => <option key={marker.markerId} value={`@marker:${marker.name}`} />)}
        </datalist>
        {kind !== "squash" && (
          <TextField label="Parent de référence si fusion" value={mainline} placeholder="Obligatoire pour une capture à plusieurs parents" onChange={(event) => setMainline(event.target.value)} />
        )}
        <div>
          <Button disabledReason={locked ? "Opération en cours" : !source.trim() ? "Indiquez la capture concernée." : undefined} onClick={preview}>
            Prévisualiser la transformation
          </Button>
        </div>
      </Card>
    </section>
  );
}
