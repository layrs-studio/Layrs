import { PageHeader, RunDetail, navigate, segment, useLocation } from "@layrs/ui";
import { useCollabContext } from "../app/collab";

/** P13b — one validation run, reached from its review. */
export function RunPage({ reviewId, runId }: { reviewId: string; runId: string }) {
  const { project, scope, base, references, people } = useCollabContext();
  const { query } = useLocation();
  const version = query.get("version") ?? undefined;
  return (
    <div className="layrs-page">
      <PageHeader title="Validation et journal" description="Résultat, preuve et journal d’une exécution sur une capture exacte." />
      <RunDetail
        key={`${scope}|${runId}`}
        scope={scope}
        transport={project.collaboration}
        runId={runId}
        references={references}
        people={people}
        onBack={() => navigate(`${base}/reviews/${segment(reviewId)}`, { query: { version, tab: "validations" } })}
      />
    </div>
  );
}
