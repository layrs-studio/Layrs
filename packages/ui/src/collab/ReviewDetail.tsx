import { useEffect, useState } from "react";
import { ArrowLeft, GitPullRequestArrow, History, Plus } from "lucide-react";
import { Button } from "../components/Button";
import { Dialog } from "../components/Dialog";
import { Badge, Banner, Card } from "../components/Feedback";
import { SelectField } from "../components/Field";
import { Tabs } from "../components/Tabs";
import { useToast } from "../components/Toast";
import { formatDateTime } from "../foundation/format";
import { exactReviewVersion } from "../native-review-context";
import type { ReviewDraftIdentity } from "../native-review-drafts";
import { PageHeader } from "../shell/AppShell";
import { ComparePanel } from "./ComparePanel";
import { captureNames, captureTitle, decisionSummary, decisionSuperseded, denied, layerName, namedCaptures, personInSentence, reviewLifecycle } from "./model";
import { CollabError, DelayedRows, TechnicalDetails, When } from "./parts";
import { ProposalForm } from "./ProposalForm";
import { ReviewDiscussion } from "./ReviewDiscussion";
import { ReviewValidations } from "./ReviewRuns";
import type { CollabCapabilities, CollabTransport, HistoryPageData, PeopleDirectory, ProjectReferences, ReviewDetailData, ReviewProposal } from "./types";
import { useScopedTasks } from "./useScopedTasks";

export type ReviewTab = "discussion" | "files" | "validations";

/**
 * P13 — one review at an exact proposal version. A version requested by a
 * link (e.g. a notification) must exist exactly; it is shown read-only with a
 * warning instead of silently jumping to the latest one.
 */
export function ReviewDetail({
  scope,
  transport,
  spaceId,
  reviewId,
  version,
  references,
  people,
  draftIdentity,
  tab,
  onTabChange,
  onOpenVersion,
  onOpenRun,
  onBack,
  capabilities,
}: {
  scope: string;
  transport: CollabTransport;
  capabilities?: CollabCapabilities;
  spaceId: string;
  reviewId: string;
  /** Exact version requested by the link; null opens the current one. */
  version: number | null;
  references: ProjectReferences;
  people?: PeopleDirectory;
  draftIdentity?: ReviewDraftIdentity;
  tab: ReviewTab;
  onTabChange: (tab: ReviewTab) => void;
  onOpenVersion: (version: number | null) => void;
  onOpenRun: (runId: string) => void;
  onBack?: () => void;
}) {
  const key = `${scope}|${reviewId}`;
  // A different exact version is a different request: late answers for the previous one are dropped.
  const tasks = useScopedTasks(`${key}|${version ?? "current"}`);
  const toast = useToast();
  const [detail, setDetail] = useState<ReviewDetailData | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [refreshError, setRefreshError] = useState<unknown>(null);
  const [proposing, setProposing] = useState(false);
  const [visited, setVisited] = useState<ReadonlySet<ReviewTab>>(() => new Set([tab]));

  async function fetchDetail(alive: () => boolean) {
    const loaded = await transport<ReviewDetailData>({ action: "review", review: reviewId });
    if (version !== null) exactReviewVersion(loaded.versions, { reviewId, version });
    if (alive()) setDetail(loaded);
  }

  function load() {
    return tasks.run("detail", fetchDetail, (cause) => {
      setDetail(null);
      setError(cause);
    });
  }

  useEffect(() => {
    setDetail(null);
    setError(null);
    setRefreshError(null);
    void load();
  }, [key, version]);

  useEffect(() => {
    setVisited((current) => (current.has(tab) ? current : new Set(current).add(tab)));
  }, [tab]);

  const current = detail?.versions.at(-1);
  const shown = detail && current ? (version === null ? current : detail.versions.find((item) => item.version === version)) : undefined;

  if (error)
    return (
      <div className="collab-review">
        {onBack && <BackButton onBack={onBack} />}
        <CollabError error={error} title="Revue non chargée" kept="Aucune autre version n’est affichée à la place de celle demandée." onRetry={() => void load()} />
      </div>
    );
  if (!detail || !current || !shown)
    return (
      <div className="collab-review">
        {onBack && <BackButton onBack={onBack} />}
        <DelayedRows active rows={4} label="Chargement de la revue…" />
      </div>
    );

  const historical = shown.version !== current.version;
  const lifecycle = reviewLifecycle(detail.lifecycle);
  const decision = decisionSummary(detail.decisionState);
  const superseded = detail.actions.filter((action) => decisionSuperseded(action, current.version)).length;
  const comments = detail.actions.filter((action) => action.version === shown.version).length;
  const integration = detail.integration;

  return (
    <div className="collab-review">
      {onBack && <BackButton onBack={onBack} />}
      <PageHeader
        title={shown.title}
        description={
          <>
            Version {shown.version} · vers {layerName(shown.layerId, references)} · proposée par {personInSentence(shown.authorPrincipalId, people)}
            {shown.createdAt ? (
              <>
                {" "}
                · <When value={shown.createdAt} />
              </>
            ) : null}
          </>
        }
        aside={
          <>
            <Badge tone={lifecycle.tone}>{lifecycle.label}</Badge>
            {detail.lifecycle === "open" && decision && <Badge tone={decision.tone}>{decision.label}</Badge>}
            {detail.versions.length > 1 && (
              <SelectField label="Version affichée" hideLabel value={String(shown.version)} onChange={(event) => onOpenVersion(Number(event.target.value) === current.version ? null : Number(event.target.value))}>
                {[...detail.versions].reverse().map((item) => (
                  <option key={item.version} value={item.version}>
                    Version {item.version}
                    {item.version === current.version ? " (actuelle)" : ""}
                  </option>
                ))}
              </SelectField>
            )}
            {detail.lifecycle === "open" && !historical && (
              <Button
                icon={<Plus aria-hidden size={16} />}
                disabledReason={denied(capabilities, "proposeReviews") ? "Votre compte ne peut pas proposer de version dans ce projet." : undefined}
                onClick={() => setProposing(true)}
              >
                Nouvelle proposition
              </Button>
            )}
          </>
        }
      />

      {historical ? (
        <Banner
          tone="warning"
          title={`Version ${shown.version} affichée en lecture seule`}
          actions={
            <Button size="sm" icon={<History aria-hidden size={14} />} onClick={() => onOpenVersion(null)}>
              Ouvrir la version actuelle
            </Button>
          }
        >
          La version actuelle est la {current.version}. Les décisions prises sur la version {shown.version} ne valent pas pour la suivante ; ses commentaires gardent leur contexte d’origine.
        </Banner>
      ) : superseded > 0 ? (
        <Banner tone="warning" title={`Nouvelle proposition : version ${current.version}`}>
          {superseded} décision{superseded > 1 ? "s" : ""} des versions précédentes ne compte{superseded > 1 ? "nt" : ""} plus. Relisez les changements et décidez à nouveau.
        </Banner>
      ) : null}
      {!historical && detail.lifecycle === "open" && detail.decisionState?.stale && (
        <Banner tone="info" title="Le projet a changé depuis cette proposition">
          Une publication, un changement de droits ou de protection est intervenu après la version {current.version}. Ses décisions doivent être
          renouvelées ; si le serveur les refuse, proposez une nouvelle version.
        </Banner>
      )}

      {refreshError ? <CollabError error={refreshError} title="Revue non actualisée" kept="L’action a été enregistrée ; l’affichage peut être en retard." onRetry={() => void tasks.run("detail", fetchDetail, setRefreshError)} /> : null}

      <Card className="collab-gate">
        <div className="collab-gate__facts">
          <div>
            <p className="collab-gate__label">Capture proposée</p>
            <p className="collab-gate__value">{captureTitle(shown.proposedStepId, references)}</p>
          </div>
          <div>
            <p className="collab-gate__label">Approbations{historical ? " (version actuelle)" : ""}</p>
            <p className="collab-gate__value">{detail.decisionState ? `${detail.decisionState.approvals} sur ${detail.decisionState.requiredApprovals} requises` : "Non communiqué"}</p>
          </div>
          <div>
            <p className="collab-gate__label">Demandes de modifications</p>
            <p className="collab-gate__value">{detail.decisionState ? String(detail.decisionState.blockingDecisions) : "Non communiqué"}</p>
          </div>
          <div>
            <p className="collab-gate__label">Intégration</p>
            <p className="collab-gate__value">
              {integration ? `Intégrée le ${formatDateTime(integration.record.integratedAt)} (version ${integration.record.version})` : detail.lifecycle === "closed" ? "Revue fermée" : "Pas encore intégrée"}
            </p>
          </div>
        </div>
        {!integration && detail.lifecycle === "open" && (
          <div className="collab-gate__integrate">
            <p className="collab-muted">
              Approuver n’intègre pas. L’intégration se fait depuis Desktop, en publiant la capture approuvée sur le Layer cible ; le serveur revérifie alors droits,
              approbations et preuves exigées.
            </p>
            <Button
              icon={<GitPullRequestArrow aria-hidden size={16} />}
              disabledReason={
                denied(capabilities, "integrateReviews")
                  ? "Votre compte ne peut pas intégrer dans ce projet."
                  : "L’intégration n’est pas disponible sur le Web : utilisez Desktop pour publier sur le Layer cible."
              }
            >
              Intégrer
            </Button>
          </div>
        )}
        <TechnicalDetails
          items={[
            { label: "Revue", value: shown.reviewId },
            { label: "Capture proposée", value: shown.proposedStepId, copy: "Copier la capture" },
            { label: "Layer cible", value: shown.layerId },
            { label: "Preuve d’intégration", value: integration?.evidenceObjectId },
            { label: "Transaction d’intégration", value: integration?.record.transactionId },
          ]}
        />
      </Card>

      <Tabs
        label="Sous-vues de la revue"
        value={tab}
        onChange={onTabChange}
        items={[
          { id: "discussion", label: "Discussion", count: comments },
          { id: "files", label: "Fichiers" },
          { id: "validations", label: "Validations" },
        ]}
      >
        <div hidden={tab !== "discussion"}>
          <ReviewDiscussion
            key={`${shown.reviewId}:${shown.version}`}
            scope={scope}
            transport={transport}
            spaceId={spaceId}
            detail={detail}
            shown={shown}
            current={current}
            people={people}
            draftIdentity={draftIdentity}
            capabilities={capabilities}
            onChanged={async () => {
              setRefreshError(null);
              await tasks.run("detail", fetchDetail, setRefreshError);
            }}
          />
        </div>
        {visited.has("files") && (
          <div hidden={tab !== "files"}>
            <ReviewFiles scope={key} transport={transport} proposal={shown} references={references} />
          </div>
        )}
        {visited.has("validations") && (
          <div hidden={tab !== "validations"}>
            <ReviewValidations scope={key} transport={transport} stepId={shown.proposedStepId} references={references} people={people} capabilities={capabilities} onOpenRun={onOpenRun} />
          </div>
        )}
      </Tabs>

      <Dialog open={proposing} onClose={() => setProposing(false)} title="Nouvelle proposition" description={`La version ${current.version + 1} remplacera la version ${current.version} : ses approbations devront être renouvelées.`}>
        {proposing && (
          <ProposalForm
            scope={scope}
            transport={transport}
            spaceId={spaceId}
            references={references}
            draftIdentity={draftIdentity}
            review={current}
            writable={detail.lifecycle === "open"}
            unavailableReason="Seule une revue ouverte accepte une nouvelle version."
            onCancel={() => setProposing(false)}
            onDone={(proposal) => {
              setProposing(false);
              toast.show({ tone: "success", title: `Version ${proposal.version} proposée`, description: "Les décisions précédentes ne comptent plus." });
              onOpenVersion(null);
              void load();
            }}
          />
        )}
      </Dialog>
    </div>
  );
}

function BackButton({ onBack }: { onBack: () => void }) {
  return (
    <div>
      <Button variant="ghost" size="sm" icon={<ArrowLeft aria-hidden size={16} />} onClick={onBack}>
        Retour aux revues
      </Button>
    </div>
  );
}

/** Changes of the proposal against a chosen base, by default its parent capture. */
function ReviewFiles({ scope, transport, proposal, references }: { scope: string; transport: CollabTransport; proposal: ReviewProposal; references: ProjectReferences }) {
  const tasks = useScopedTasks(`${scope}|files|${proposal.version}`);
  const [parents, setParents] = useState<string[] | null>(null);
  const [before, setBefore] = useState("");
  const [error, setError] = useState<unknown>(null);
  const target = references.layers.find((layer) => layer.layerId === proposal.layerId);
  const heads = (target?.heads ?? []).filter((head) => head !== proposal.proposedStepId);

  useEffect(() => {
    setParents(null);
    setError(null);
    void tasks.run(
      "parents",
      async (alive) => {
        const result = await transport<HistoryPageData>({
          action: "stepHistory",
          anchor: proposal.proposedStepId,
          cursor: null,
          limit: 1,
          path: null,
          filter: { actorId: null, since: null, until: null },
        });
        if (!alive()) return;
        const list = result.steps.find((step) => step.stepId === proposal.proposedStepId)?.parentStepIds ?? [];
        setParents(list);
        setBefore((current) => current || list[0] || heads[0] || "");
      },
      (cause) => {
        setError(cause);
        setParents([]);
        setBefore((current) => current || heads[0] || "");
      },
    );
  }, [proposal.proposedStepId]);

  const options = [
    ...(parents ?? []).map((parent, index) => ({ stepId: parent, label: `${parents!.length > 1 ? `Capture parente ${index + 1}` : "Capture parente"}${captureNames(parent, references)[0] ? ` · ${captureNames(parent, references)[0]}` : ""}` })),
    ...heads.map((head) => ({ stepId: head, label: `Dernière capture de ${target?.name ?? "la cible"}` })),
    ...namedCaptures(references).filter((option) => option.stepId !== proposal.proposedStepId),
  ].filter((option, index, all) => all.findIndex((other) => other.stepId === option.stepId) === index);

  return (
    <div className="collab-review-files" role="region" aria-label="Fichiers proposés">
      {error && !parents?.length ? (
        <Banner tone="info">La capture parente n’a pas pu être lue avec vos droits ; comparez avec une capture nommée.</Banner>
      ) : null}
      {options.length > 0 && (
        <SelectField label="Comparer avec" value={before} onChange={(event) => setBefore(event.target.value)}>
          {options.map((option) => (
            <option key={option.stepId} value={option.stepId}>
              {option.label}
            </option>
          ))}
        </SelectField>
      )}
      {parents === null && !error ? (
        <DelayedRows active rows={3} label="Préparation de la comparaison…" />
      ) : before ? (
        <ComparePanel scope={scope} transport={transport} before={before} after={proposal.proposedStepId} beforeLabel={captureTitle(before, references, "Avant")} afterLabel={`Proposition v${proposal.version}`} />
      ) : (
        <Banner tone="info">Aucune capture de comparaison disponible pour cette proposition.</Banner>
      )}
    </div>
  );
}
