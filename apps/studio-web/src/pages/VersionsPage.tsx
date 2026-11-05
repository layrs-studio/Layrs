import { PageHeader, Releases, navigate, useLocation, type ReleasesTab } from "@layrs/ui";
import { useCollabContext } from "../app/collab";

/** P14 — publishable versions and markers; the tab is kept in the address. */
export function VersionsPage() {
  const { project, scope, base, references, people, capabilities } = useCollabContext();
  const { query } = useLocation();
  const tab: ReleasesTab = query.get("tab") === "markers" ? "markers" : "releases";
  return (
    <div className="layrs-page">
      <PageHeader title="Versions et repères" description="Un repère nomme une capture ; une version publiée est rendue disponible à l’équipe." />
      <Releases
        key={scope}
        scope={scope}
        transport={project.collaboration}
        references={references}
        people={people}
        capabilities={capabilities}
        tab={tab}
        onTabChange={(next) => navigate(`${base}/versions`, { replace: true, query: { tab: next === "releases" ? undefined : next } })}
      />
    </div>
  );
}
