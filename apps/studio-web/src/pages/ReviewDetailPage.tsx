import { ReviewDetail, navigate, segment, useLocation, type ReviewTab } from "@layrs/ui";
import { useCollabContext } from "../app/collab";

const tabs: ReviewTab[] = ["discussion", "files", "validations"];

/**
 * P13 — one review. `?version=` pins an exact proposal version (e.g. from a
 * notification); without it the current version opens. The tab is kept in
 * the address so the back button and reloads restore it.
 */
export function ReviewDetailPage({ reviewId }: { reviewId: string }) {
  const { project, scope, base, references, people, capabilities, draftIdentity } = useCollabContext();
  const { query } = useLocation();
  const requested = Number(query.get("version"));
  const version = Number.isSafeInteger(requested) && requested > 0 ? requested : null;
  const tab = tabs.includes(query.get("tab") as ReviewTab) ? (query.get("tab") as ReviewTab) : "discussion";
  const path = `${base}/reviews/${segment(reviewId)}`;
  const withQuery = (next: { version?: number | null; tab?: ReviewTab }) => {
    const nextVersion = next.version === undefined ? version : next.version;
    const nextTab = next.tab ?? tab;
    return { version: nextVersion ? String(nextVersion) : undefined, tab: nextTab === "discussion" ? undefined : nextTab };
  };
  return (
    <div className="layrs-page">
      <ReviewDetail
        key={`${scope}|${reviewId}`}
        scope={scope}
        transport={project.collaboration}
        spaceId={project.spaceId}
        reviewId={reviewId}
        version={version}
        references={references}
        people={people}
        draftIdentity={draftIdentity}
        capabilities={capabilities}
        tab={tab}
        onTabChange={(next) => navigate(path, { replace: true, query: withQuery({ tab: next }) })}
        onOpenVersion={(next) => navigate(path, { query: withQuery({ version: next }) })}
        onOpenRun={(runId) => navigate(`${path}/runs/${segment(runId)}`, { query: { version: version ? String(version) : undefined } })}
        onBack={() => navigate(`${base}/reviews`)}
      />
    </div>
  );
}
