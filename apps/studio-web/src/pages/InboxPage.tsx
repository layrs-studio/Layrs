import { useCallback, useEffect, useMemo, useState } from "react";
import { Activity } from "lucide-react";
import {
  EmptyState,
  InvitationInbox,
  Loading,
  PageHeader,
  ProjectActivity,
  SelectField,
  Tabs,
  navigate,
  segment,
  useLocation,
  type SpaceCatalogEntry,
  type SpaceCatalogPage,
} from "@layrs/ui";
import { administrationTransport } from "../app/transports";
import { rememberProject } from "../app/project";
import { useSession } from "../app/session";
import { openProject } from "./ProjectsPage";

type Tab = "invitations" | "activity";

/**
 * P18 · Boîte de réception: invitations received by this account, and the
 * activity of one selected project. No cross-project badge: an aggregate
 * would need a dedicated server contract.
 */
export function InboxPage({ section }: { section?: string }) {
  const { api, principal } = useSession();
  const { query } = useLocation();
  const tab: Tab = section === "activity" ? "activity" : "invitations";
  const [projects, setProjects] = useState<SpaceCatalogEntry[] | null>(null);
  const [more, setMore] = useState(false);
  const scope = `${location.origin}:${principal?.principalId ?? ""}`;
  const instance = useMemo(() => administrationTransport(api, ""), [api]);

  useEffect(() => {
    let live = true;
    setProjects(null);
    api<SpaceCatalogPage>("/v3/spaces?limit=100")
      .then((page) => {
        if (!live) return;
        page.spaces.forEach((space) => rememberProject(space.spaceId, space.name));
        setProjects(page.spaces);
        setMore(page.hasMore);
      })
      .catch(() => live && setProjects([]));
    return () => {
      live = false;
    };
  }, [api, scope]);

  const names = useMemo(() => new Map((projects ?? []).map((project) => [project.spaceId, project.name])), [projects]);
  const projectName = useCallback((spaceId: string) => names.get(spaceId), [names]);
  const selected = query.get("project") ?? "";
  const chosen = (projects ?? []).find((project) => project.spaceId === selected) ?? null;

  return (
    <div className="layrs-page">
      <PageHeader title="Boîte de réception" description="Ce qui attend votre réponse : invitations reçues et activité d’un projet." />
      <Tabs
        label="Boîte de réception"
        value={tab}
        onChange={(next) => navigate(next === "activity" ? "/inbox/activity" : "/inbox", { query: next === "activity" && selected ? { project: selected } : undefined })}
        items={[
          { id: "invitations", label: "Invitations" },
          { id: "activity", label: "Activité du projet" },
        ]}
      >
        {tab === "invitations" ? (
          <InvitationInbox
            scope={scope}
            administration={instance}
            projectName={projectName}
            onOpenProject={(spaceId) => openProject(spaceId)}
          />
        ) : !projects ? (
          <Loading label="Lecture de vos projets…" />
        ) : !projects.length ? (
          <EmptyState icon={<Activity size={22} />} title="Aucun projet accessible">
            Acceptez une invitation pour suivre l’activité d’un projet.
          </EmptyState>
        ) : (
          <div className="team-panel">
            <div className="team-panel--narrow">
              <SelectField
                label="Projet"
                value={selected}
                hint={more ? "Les 100 premiers projets sont proposés." : undefined}
                onChange={(event) => navigate("/inbox/activity", { query: { project: event.target.value || undefined } })}
              >
                <option value="">Choisir un projet…</option>
                {projects.map((project) => (
                  <option key={project.spaceId} value={project.spaceId}>
                    {project.name} · {project.workspace.name}
                  </option>
                ))}
              </SelectField>
            </div>
            {chosen ? (
              <ActivityOf key={chosen.spaceId} project={chosen} scope={`${scope}:${chosen.spaceId}`} />
            ) : (
              <EmptyState compact icon={<Activity size={22} />} title="Choisissez un projet">
                L’activité est lue projet par projet ; rien n’est marqué comme lu sans votre action.
              </EmptyState>
            )}
          </div>
        )}
      </Tabs>
    </div>
  );
}

function ActivityOf({ project, scope }: { project: SpaceCatalogEntry; scope: string }) {
  const { api } = useSession();
  const transport = useMemo(() => administrationTransport(api, project.spaceId), [api, project.spaceId]);
  return (
    <ProjectActivity
      scope={scope}
      administration={transport}
      projectName={project.name}
      onOpenReview={(context) =>
        navigate(`/projects/${segment(project.spaceId)}/reviews/${segment(context.reviewId)}`, { query: { version: String(context.version) } })
      }
    />
  );
}
