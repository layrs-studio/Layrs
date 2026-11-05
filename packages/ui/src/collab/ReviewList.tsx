import { useEffect, useMemo, useState } from "react";
import { MessageSquare, Plus, RefreshCw, Search } from "lucide-react";
import { Button } from "../components/Button";
import { Dialog } from "../components/Dialog";
import { Avatar, Badge, Banner, EmptyState } from "../components/Feedback";
import { TextField } from "../components/Field";
import { SegmentedControl } from "../components/SegmentedControl";
import { useToast } from "../components/Toast";
import type { ReviewDraftIdentity } from "../native-review-drafts";
import { decisionSummary, denied, layerName, personAvatarName, personInSentence, reviewLifecycle } from "./model";
import { CollabError, DelayedRows, When } from "./parts";
import { ProposalForm } from "./ProposalForm";
import type { CollabCapabilities, CollabTransport, PeopleDirectory, ProjectReferences, ReviewProposal, ReviewSummary } from "./types";
import { useScopedTasks } from "./useScopedTasks";

export type ReviewFilter = "open" | "integrated" | "closed" | "all";

const PAGE_SIZE = 50;

/**
 * P12 — the queue of collective decisions. Counts describe the reviews loaded
 * so far and say so; nothing implies an assignment that does not exist.
 */
export function ReviewList({
  scope,
  transport,
  spaceId,
  references,
  people,
  draftIdentity,
  onOpenReview,
  filter: controlledFilter,
  onFilterChange,
  capabilities,
}: {
  scope: string;
  capabilities?: CollabCapabilities;
  transport: CollabTransport;
  spaceId: string;
  references: ProjectReferences;
  people?: PeopleDirectory;
  draftIdentity?: ReviewDraftIdentity;
  onOpenReview: (reviewId: string) => void;
  filter?: ReviewFilter;
  onFilterChange?: (filter: ReviewFilter) => void;
}) {
  const tasks = useScopedTasks(scope);
  const toast = useToast();
  const [reviews, setReviews] = useState<ReviewSummary[] | null>(null);
  const [cursor, setCursor] = useState<string | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [localFilter, setLocalFilter] = useState<ReviewFilter>("open");
  const [query, setQuery] = useState("");
  const [proposing, setProposing] = useState(false);
  const filter = controlledFilter ?? localFilter;

  function load(next = false) {
    return tasks.run(
      "reviews",
      async (alive) => {
        setError(null);
        const result = await transport<{ reviews: ReviewSummary[]; nextCursor: string | null }>({ action: "listReviewsPage", cursor: next ? cursor : null, limit: PAGE_SIZE });
        if (!alive()) return;
        setReviews((current) => {
          if (!next || !current) return result.reviews;
          const known = new Set(current.map((review) => review.proposal.reviewId));
          return [...current, ...result.reviews.filter((review) => !known.has(review.proposal.reviewId))];
        });
        setCursor(result.nextCursor);
      },
      setError,
    );
  }

  useEffect(() => {
    setReviews(null);
    setCursor(null);
    setError(null);
    if (!denied(capabilities, "readReviews")) void load();
  }, [scope]);

  const visible = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase("fr");
    return (reviews ?? []).filter(
      (review) => (filter === "all" || review.lifecycle === filter) && (!needle || review.proposal.title.toLocaleLowerCase("fr").includes(needle)),
    );
  }, [reviews, filter, query]);
  const count = (value: ReviewFilter) => (reviews ?? []).filter((review) => value === "all" || review.lifecycle === value).length;
  const loading = tasks.isBusy("reviews");
  const proposeReason = denied(capabilities, "proposeReviews") ? "Votre compte ne peut pas proposer de revue dans ce projet." : undefined;

  if (denied(capabilities, "readReviews"))
    return (
      <section className="collab-reviews" aria-label="Revues du projet">
        <Banner tone="warning" title="Revues non accessibles">
          Votre compte ne peut pas lire les revues de ce projet{capabilities?.readScopedReviews ? " ; les revues à portée limitée restent disponibles depuis Desktop" : ""}.
        </Banner>
      </section>
    );

  return (
    <section className="collab-reviews" aria-label="Revues du projet">
      <div className="collab-toolbar">
        <SegmentedControl
          label="État des revues"
          value={filter}
          onChange={(value) => (onFilterChange ? onFilterChange(value) : setLocalFilter(value))}
          options={[
            { value: "open", label: `Ouvertes${reviews ? ` (${count("open")})` : ""}` },
            { value: "integrated", label: `Intégrées${reviews ? ` (${count("integrated")})` : ""}` },
            { value: "closed", label: `Fermées${reviews ? ` (${count("closed")})` : ""}` },
            { value: "all", label: "Toutes" },
          ]}
        />
        <TextField label="Rechercher un titre" hideLabel placeholder="Rechercher un titre…" value={query} trailing={<Search aria-hidden size={16} className="collab-field-icon" />} onChange={(event) => setQuery(event.target.value)} />
        <div className="collab-toolbar__end">
          <Button icon={<RefreshCw aria-hidden size={16} />} busy={loading} busyLabel="Actualisation…" onClick={() => void load()}>
            Actualiser
          </Button>
          <Button variant="primary" icon={<Plus aria-hidden size={16} />} disabledReason={proposeReason} onClick={() => setProposing(true)}>
            Proposer une revue
          </Button>
        </div>
      </div>
      {reviews && (
        <p className="collab-muted">
          Nombres calculés sur les {reviews.length} revue{reviews.length > 1 ? "s" : ""} chargée{reviews.length > 1 ? "s" : ""}
          {cursor ? " ; d’autres revues restent à charger." : "."}
        </p>
      )}
      {error ? <CollabError error={error} title="Revues non chargées" onRetry={() => void load()} /> : null}
      <DelayedRows active={loading && !reviews} label="Chargement des revues…" />
      {reviews && !reviews.length && (
        <EmptyState
          icon={<MessageSquare size={22} />}
          title="Aucune revue pour l’instant"
          actions={
            <Button variant="primary" icon={<Plus aria-hidden size={16} />} disabledReason={proposeReason} onClick={() => setProposing(true)}>
              Proposer une revue
            </Button>
          }
        >
          Une revue réunit discussion et décisions sur une capture publiée précise, avant son intégration dans un Layer.
        </EmptyState>
      )}
      {reviews && reviews.length > 0 && !visible.length && (
        <EmptyState compact title="Aucune revue ne correspond">
          Changez d’état ou de recherche{cursor ? ", ou chargez les revues suivantes" : ""}.
        </EmptyState>
      )}
      {visible.length > 0 && (
        <ul className="collab-cards" aria-label="Liste des revues">
          {visible.map((review) => {
            const lifecycle = reviewLifecycle(review.lifecycle);
            const decision = decisionSummary(review.decisionState);
            return (
              <li key={review.proposal.reviewId}>
                <button type="button" className="collab-review-row" onClick={() => onOpenReview(review.proposal.reviewId)}>
                  <Avatar name={personAvatarName(review.proposal.authorPrincipalId, people)} size={32} />
                  <span className="collab-review-row__text">
                    <span className="collab-review-row__title">{review.proposal.title}</span>
                    <span className="collab-row__meta">
                      Proposée par {personInSentence(review.proposal.authorPrincipalId, people)} · vers {layerName(review.proposal.layerId, references)} · version {review.proposal.version}
                      {review.proposal.createdAt ? (
                        <>
                          {" "}
                          · <When value={review.proposal.createdAt} />
                        </>
                      ) : null}
                    </span>
                  </span>
                  <span className="collab-badges">
                    <Badge tone={lifecycle.tone}>{lifecycle.label}</Badge>
                    {decision && review.lifecycle === "open" && (
                      <Badge tone={decision.tone}>
                        {decision.label} · {review.decisionState!.approvals}/{review.decisionState!.requiredApprovals}
                      </Badge>
                    )}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
      {cursor && (
        <div className="collab-pager">
          <Button size="sm" busy={loading} busyLabel="Chargement…" onClick={() => void load(true)}>
            Revues suivantes
          </Button>
        </div>
      )}
      <Dialog open={proposing} onClose={() => setProposing(false)} title="Proposer une revue" description="Choisissez la capture publiée et le Layer où elle serait intégrée." size="md">
        {proposing && (
          <ProposalForm
            scope={scope}
            transport={transport}
            spaceId={spaceId}
            references={references}
            draftIdentity={draftIdentity}
            onCancel={() => setProposing(false)}
            onDone={(proposal: ReviewProposal) => {
              setProposing(false);
              toast.show({ tone: "success", title: "Revue créée", description: proposal.title });
              void load();
              onOpenReview(proposal.reviewId);
            }}
          />
        )}
      </Dialog>
    </section>
  );
}
