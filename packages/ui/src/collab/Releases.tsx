import { useEffect, useRef, useState } from "react";
import { Bookmark, GitBranch, RefreshCw, Send, ShieldOff, Tag } from "lucide-react";
import { Button } from "../components/Button";
import { Dialog } from "../components/Dialog";
import { Badge, Banner, Card, EmptyState } from "../components/Feedback";
import { TextAreaField } from "../components/Field";
import { Tabs } from "../components/Tabs";
import { useToast } from "../components/Toast";
import { formatDateTime } from "../foundation/format";
import { captureTitle, denied, personInSentence } from "./model";
import { CollabError, DelayedRows, TechnicalDetails, When } from "./parts";
import type { CollabCapabilities, CollabTransport, PeopleDirectory, ProjectReferences, PublicationHistory, ReleaseEntry } from "./types";
import { useScopedTasks } from "./useScopedTasks";

export type ReleasesTab = "releases" | "markers";

function publicationState(history: PublicationHistory | null) {
  if (!history) return null;
  if (history.activePublicationId) return { label: "Publication active", tone: "success" as const };
  if (history.events.at(-1)?.event.kind === "revoked") return { label: "Publication révoquée", tone: "warning" as const };
  return { label: "Non publiée", tone: "info" as const };
}

/**
 * P14 — named states of the project. A marker names a capture; a release can
 * be published to the team, and a publication can be revoked. Publication
 * designates one exact version; revocation keeps its manifest and history.
 */
export function Releases({
  scope,
  transport,
  references,
  people,
  tab,
  onTabChange,
  capabilities,
}: {
  scope: string;
  transport: CollabTransport;
  capabilities?: CollabCapabilities;
  references: ProjectReferences;
  people?: PeopleDirectory;
  tab: ReleasesTab;
  onTabChange: (tab: ReleasesTab) => void;
}) {
  return (
    <section className="collab-releases" aria-label="Versions et repères">
      <Tabs
        label="Versions et repères"
        value={tab}
        onChange={onTabChange}
        items={[
          { id: "releases", label: "Versions publiables" },
          { id: "markers", label: "Repères", count: references.markers.length },
        ]}
      >
        {tab === "releases" ? (
          denied(capabilities, "readReleases") ? (
            <Banner tone="warning" title="Versions non accessibles">
              Votre compte ne peut pas consulter les publications de versions de ce projet.
            </Banner>
          ) : (
            <ReleaseList scope={scope} transport={transport} references={references} people={people} capabilities={capabilities} />
          )
        ) : (
          <MarkerList references={references} />
        )}
      </Tabs>
      <LayerHeads references={references} />
    </section>
  );
}

function ReleaseList({
  scope,
  transport,
  references,
  people,
  capabilities,
}: {
  scope: string;
  transport: CollabTransport;
  references: ProjectReferences;
  people?: PeopleDirectory;
  capabilities?: CollabCapabilities;
}) {
  const tasks = useScopedTasks(scope);
  const toast = useToast();
  const [releases, setReleases] = useState<ReleaseEntry[] | null>(null);
  const [selected, setSelected] = useState("");
  const [history, setHistory] = useState<PublicationHistory | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [actionError, setActionError] = useState<unknown>(null);
  const [revoking, setRevoking] = useState(false);
  const [reason, setReason] = useState("");
  const pending = useRef({ signature: "", requestId: "" });

  function loadList() {
    return tasks.run(
      "list",
      async (alive) => {
        setError(null);
        const response = await transport<{ releases: ReleaseEntry[] }>({ action: "listReleases" });
        if (!alive()) return;
        setReleases(response.releases);
        const keep = response.releases.some((release) => release.record.releaseId === selected);
        const next = keep ? selected : (response.releases[0]?.record.releaseId ?? "");
        setSelected(next);
        if (!keep) setHistory(null);
      },
      (cause) => {
        setReleases(null);
        setError(cause);
      },
    );
  }

  function loadHistory(release: string) {
    return tasks.run(
      "history",
      async (alive) => {
        const value = await transport<PublicationHistory>({ action: "releasePublications", release });
        if (alive()) setHistory(value.releaseId === release ? value : null);
      },
      (cause) => {
        setHistory(null);
        setActionError(cause);
      },
    );
  }

  useEffect(() => {
    setReleases(null);
    setSelected("");
    setHistory(null);
    setError(null);
    setActionError(null);
    pending.current = { signature: "", requestId: "" };
    void loadList();
  }, [scope]);

  useEffect(() => {
    setHistory(null);
    setActionError(null);
    setReason("");
    if (selected) void loadHistory(selected);
  }, [selected]);

  const release = releases?.find((item) => item.record.releaseId === selected) ?? null;
  const state = publicationState(history);

  function mutate(revoke: boolean) {
    if (!history || !release || history.releaseId !== release.record.releaseId) return;
    const command = { action: revoke ? "revokeRelease" : "publishRelease", release: release.record.releaseId, ...(revoke ? { publication: history.activePublicationId } : {}) };
    const body = { expectedLastEventId: history.lastEventObjectId, ...(revoke ? { reason } : {}) };
    const signature = JSON.stringify({ command, body });
    if (pending.current.signature !== signature) pending.current = { signature, requestId: crypto.randomUUID() };
    void tasks.run(
      "mutation",
      async (alive) => {
        setActionError(null);
        await transport({ ...command, request: { ...body, requestId: pending.current.requestId } });
        pending.current = { signature: "", requestId: "" };
        if (!alive()) return;
        setRevoking(false);
        setReason("");
        toast.show({
          tone: "success",
          title: revoke ? `Publication de ${release.record.version} révoquée` : `Version ${release.record.version} publiée`,
          description: revoke ? "Le manifeste et l’historique restent conservés." : "Publication confirmée par le serveur.",
        });
        const value = await transport<PublicationHistory>({ action: "releasePublications", release: release.record.releaseId });
        if (alive()) setHistory(value);
      },
      setActionError,
    );
  }

  const busy = tasks.isBusy("mutation");

  return (
    <div className="collab-split collab-split--releases">
      <div className="collab-split__list">
        <div className="collab-toolbar">
          <Button size="sm" icon={<RefreshCw aria-hidden size={14} />} busy={tasks.isBusy("list")} busyLabel="Actualisation…" onClick={() => void loadList()}>
            Actualiser
          </Button>
        </div>
        {error ? <CollabError error={error} title="Versions non chargées" onRetry={() => void loadList()} /> : null}
        <DelayedRows active={tasks.isBusy("list") && !releases} label="Chargement des versions…" />
        {releases && !releases.length && (
          <EmptyState compact icon={<Tag size={20} />} title="Aucune version publiable">
            Les versions se créent depuis Desktop sur une capture, puis se publient ici pour l’équipe.
          </EmptyState>
        )}
        {releases && releases.length > 0 && (
          <ul className="collab-rows" aria-label="Versions du projet">
            {releases.map(({ record }) => (
              <li key={record.releaseId}>
                <button type="button" className="collab-row" aria-current={record.releaseId === selected ? "true" : undefined} onClick={() => setSelected(record.releaseId)}>
                  <span className="collab-row__icon" aria-hidden>
                    <Tag size={18} />
                  </span>
                  <span className="collab-row__text">
                    <span className="collab-row__title">{record.version}</span>
                    <span className="collab-row__meta">{record.title && record.title !== record.version ? record.title : captureTitle(record.targetStepId, references)}</span>
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
      <div className="collab-split__detail" role="region" aria-label="Publication de la version">
        {release ? (
          <div className="collab-release">
            <header className="collab-release__head">
              <h2>{release.record.version}</h2>
              {state && <Badge tone={state.tone}>{state.label}</Badge>}
            </header>
            {release.record.title && release.record.title !== release.record.version && <p>{release.record.title}</p>}
            {release.record.notes && <p className="collab-muted collab-thread__text">{release.record.notes}</p>}
            <dl className="collab-facts">
              <div>
                <dt>Cible</dt>
                <dd>{captureTitle(release.record.targetStepId, references)}</dd>
              </div>
              {release.record.createdAt && (
                <div>
                  <dt>Créée</dt>
                  <dd>{formatDateTime(release.record.createdAt)}</dd>
                </div>
              )}
            </dl>
            <TechnicalDetails items={[{ label: "Capture cible", value: release.record.targetStepId, copy: "Copier la capture" }, { label: "Version", value: release.record.releaseId }]} />
            {actionError ? <CollabError error={actionError} title={history?.activePublicationId ? "Révocation non appliquée" : "Publication non appliquée"} kept="Rien n’a changé côté serveur si le refus est confirmé ; l’historique ci-dessous fait foi." /> : null}
            <DelayedRows active={tasks.isBusy("history") && !history} rows={2} label="Chargement des publications…" />
            {history && (
              <>
                <div className="collab-actions">
                  {history.activePublicationId ? (
                    <Button
                      variant="danger"
                      icon={<ShieldOff aria-hidden size={16} />}
                      disabledReason={denied(capabilities, "revokeReleases") ? "Votre compte ne peut pas révoquer de publication dans ce projet." : busy ? "Opération en cours." : undefined}
                      onClick={() => setRevoking(true)}
                    >
                      Révoquer la publication…
                    </Button>
                  ) : (
                    <Button
                      variant="primary"
                      icon={<Send aria-hidden size={16} />}
                      busy={busy}
                      busyLabel="Publication…"
                      disabledReason={denied(capabilities, "publishReleases") ? "Votre compte ne peut pas publier de version dans ce projet." : undefined}
                      onClick={() => mutate(false)}
                    >
                      Publier cette version
                    </Button>
                  )}
                </div>
                <p className="collab-muted">
                  Publier rend cette version exacte disponible à l’équipe, sous réserve des protections du projet. Ce n’est pas une diffusion publique sur Internet.
                </p>
                <h3 className="collab-subtitle">Historique des publications</h3>
                {history.events.length ? (
                  <ol className="collab-events" aria-label="Historique des publications">
                    {history.events.map((entry) => (
                      <li key={entry.eventObjectId}>
                        <strong>{entry.event.kind === "published" ? "Publiée" : "Révoquée"}</strong> <When value={entry.event.occurredAt} /> · par{" "}
                        {personInSentence(entry.event.actorId, people)}
                        {entry.event.reason && <p className="collab-thread__text">Motif : {entry.event.reason}</p>}
                      </li>
                    ))}
                  </ol>
                ) : (
                  <p className="collab-muted">Jamais publiée.</p>
                )}
              </>
            )}
            <Dialog
              open={revoking}
              onClose={() => setRevoking(false)}
              title={`Révoquer la publication de ${release.record.version}`}
              description={`L’équipe ne verra plus ${release.record.version} comme version publiée. Le manifeste, les fichiers et l’historique de la version sont conservés ; elle pourra être publiée de nouveau.`}
              footer={
                <>
                  <Button onClick={() => setRevoking(false)}>Conserver la publication</Button>
                  <Button variant="danger" busy={busy} busyLabel="Révocation…" disabledReason={!reason.trim() ? "Indiquez un motif." : undefined} onClick={() => mutate(true)}>
                    Révoquer {release.record.version}
                  </Button>
                </>
              }
            >
              <TextAreaField label="Motif de révocation" value={reason} maxLength={4096} onChange={(event) => setReason(event.target.value)} />
              {actionError ? <CollabError error={actionError} title="Révocation non appliquée" /> : null}
            </Dialog>
          </div>
        ) : (
          <EmptyState compact icon={<Tag size={20} />} title="Aucune version sélectionnée">
            Choisissez une version pour voir sa publication.
          </EmptyState>
        )}
      </div>
    </div>
  );
}

function MarkerList({ references }: { references: ProjectReferences }) {
  if (!references.markers.length)
    return (
      <EmptyState compact icon={<Bookmark size={20} />} title="Aucun repère publié">
        Un repère nomme une capture importante. Il se crée depuis Desktop et n’est pas une version rendue disponible à l’équipe.
      </EmptyState>
    );
  return (
    <Card>
      <ul className="collab-points" aria-label="Repères publiés">
        {references.markers.map((marker) => (
          <li key={marker.markerId || `${marker.name}:${marker.targetStepId}`} className="collab-points__item">
            <Bookmark aria-hidden size={18} />
            <div className="collab-points__text">
              <h3>{marker.name}</h3>
              {marker.title && <p>{marker.title}</p>}
              {marker.notes && <p className="collab-muted collab-thread__text">{marker.notes}</p>}
              <p className="collab-muted">
                Cible : {captureTitle(marker.targetStepId, references)}
                {marker.createdAt ? (
                  <>
                    {" "}
                    · <When value={marker.createdAt} />
                  </>
                ) : null}
              </p>
              <TechnicalDetails items={[{ label: "Capture", value: marker.targetStepId, copy: "Copier la capture" }, { label: "Repère", value: marker.markerId }]} />
            </div>
            <Badge>Repère</Badge>
          </li>
        ))}
      </ul>
    </Card>
  );
}

function LayerHeads({ references }: { references: ProjectReferences }) {
  if (!references.layers.length) return null;
  return (
    <section aria-label="Layers publiés" className="collab-layers">
      <h2 className="collab-subtitle">Layers publiés</h2>
      <Card>
        <ul className="collab-points">
          {references.layers.map((layer) => (
            <li key={layer.layerId} className="collab-points__item">
              <GitBranch aria-hidden size={18} />
              <div className="collab-points__text">
                <h3>{layer.name}</h3>
                <p className="collab-muted">
                  {layer.heads.length === 0 ? "Aucune capture publiée" : layer.heads.length === 1 ? "Une dernière capture publiée" : `${layer.heads.length} têtes concurrentes`}
                </p>
                <TechnicalDetails items={layer.heads.map((head, index) => ({ label: layer.heads.length > 1 ? `Tête ${index + 1}` : "Dernière capture", value: head, copy: "Copier la capture" }))} />
              </div>
              {layer.heads.length > 1 && <Badge tone="warning">Plusieurs têtes : intégration nécessaire</Badge>}
            </li>
          ))}
        </ul>
      </Card>
      {references.restricted && <Banner tone="info">Accès limité : certains contenus de ce projet ne vous sont pas ouverts.</Banner>}
    </section>
  );
}
