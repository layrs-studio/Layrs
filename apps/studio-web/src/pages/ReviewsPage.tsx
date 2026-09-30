import { PageHeader, ReviewList, navigate, segment, useLocation, type ReviewFilter } from "@layrs/ui";
import { useCollabContext } from "../app/collab";

const filters: ReviewFilter[] = ["open", "integrated", "closed", "all"];

/** P12 — the project's review queue. The state filter is kept in the address. */
export function ReviewsPage() {
  const { project, scope, base, references, people, capabilities, draftIdentity } = useCollabContext();
  const { query } = useLocation();
  const state = query.get("state");
  const filter = filters.includes(state as ReviewFilter) ? (state as ReviewFilter) : "open";
  return (
    <div className="layrs-page">
      <PageHeader title="Revues" description="Propositions, discussions et décisions sur des versions précises." />
      <ReviewList
        key={scope}
        scope={scope}
        transport={project.collaboration}
        spaceId={project.spaceId}
        references={references}
        people={people}
        draftIdentity={draftIdentity}
        capabilities={capabilities}
        filter={filter}
        onFilterChange={(next) => navigate(`${base}/reviews`, { replace: true, query: { state: next === "open" ? undefined : next } })}
        onOpenReview={(reviewId) => navigate(`${base}/reviews/${segment(reviewId)}`)}
      />
    </div>
  );
}
