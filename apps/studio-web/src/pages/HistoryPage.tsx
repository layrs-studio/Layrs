import { CaptureDetail, PageHeader, ProjectHistory, navigate, segment, useLocation, type HistoryFilters } from "@layrs/ui";
import { useCollabContext } from "../app/collab";

const filterKeys = ["from", "path", "since", "until", "actor"] as const;

function readFilters(query: URLSearchParams): HistoryFilters {
  return { from: query.get("from") ?? "", path: query.get("path") ?? "", since: query.get("since") ?? "", until: query.get("until") ?? "", actor: query.get("actor") ?? "" };
}

function filterQuery(filters: HistoryFilters) {
  return Object.fromEntries(filterKeys.map((key) => [key, filters[key] || undefined]));
}

/**
 * P07 — published history, and P08 read-only capture detail. Filters live in
 * the address so they survive a detail visit, a reload or the back button.
 */
export function HistoryPage({ capture }: { capture?: string }) {
  const { project, scope, base, references, people } = useCollabContext();
  const { query } = useLocation();
  const filters = readFilters(query);

  if (capture)
    return (
      <div className="layrs-page">
        <PageHeader title="Capture publiée" description="Résumé, provenance et comparaison. Consultation seule sur le Web." />
        <CaptureDetail
          key={`${scope}|${capture}`}
          scope={scope}
          transport={project.collaboration}
          references={references}
          people={people}
          stepId={capture}
          onBack={() => navigate(`${base}/history`, { query: filterQuery(filters) })}
        />
      </div>
    );

  return (
    <div className="layrs-page">
      <PageHeader title="Historique" description="Captures publiées et leur provenance, sans copier d’identifiant." />
      <ProjectHistory
        key={scope}
        scope={scope}
        transport={project.collaboration}
        references={references}
        people={people}
        filters={filters}
        onFiltersChange={(next) => navigate(`${base}/history`, { replace: true, query: filterQuery(next) })}
        onOpenCapture={(stepId) => navigate(`${base}/history/${segment(stepId)}`, { query: filterQuery(filters) })}
      />
    </div>
  );
}
