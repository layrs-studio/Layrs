import { useCallback, useState } from "react";
import { Badge, InvitationsPanel, Loading, MembersPanel, PageHeader, Tabs, TeamsPanel, navigate, segment, useDirectory } from "@layrs/ui";
import { useTeamContext } from "./team-context";

type Tab = "members" | "invitations" | "teams";
const tabs: { id: Tab; label: string }[] = [
  { id: "members", label: "Membres" },
  { id: "invitations", label: "Invitations" },
  { id: "teams", label: "Équipes" },
];

/** P15 · Équipe et accès : personnes. Sub-views are routes: /team, /team/invitations, /team/teams. */
export function TeamPage({ section }: { section?: string }) {
  const context = useTeamContext();
  const tab: Tab = section === "invitations" || section === "teams" ? section : "members";
  return (
    <div className="layrs-page">
      <PageHeader
        title="Équipe et accès"
        description="Qui travaille sur ce projet, avec quel rôle, et qui y est invité."
        aside={
          context.capabilities.flags &&
          (context.capabilities.flags.manageMembers ? (
            <Badge tone="accent">Vous administrez ce projet</Badge>
          ) : context.capabilities.flags.managePolicy ? (
            <Badge tone="warning">Règles personnalisées : rôles suspendus</Badge>
          ) : (
            <Badge>Consultation</Badge>
          ))
        }
      />
      {context.capabilities.loading && !context.capabilities.value ? <Loading label="Lecture de vos droits sur le projet…" /> : <TeamContent tab={tab} context={context} />}
    </div>
  );
}

function TeamContent({ tab, context }: { tab: Tab; context: ReturnType<typeof useTeamContext> }) {
  const { project, scope, capabilities, policy, self, instanceAdmin } = context;
  const [inviteRequested, setInviteRequested] = useState(false);
  const directory = useDirectory({
    scope,
    administration: project.administration,
    policy,
    instanceAdmin,
    self,
    administer: capabilities.flags ? capabilities.flags.managePolicy : null,
  });
  const key = segment(project.spaceId);
  const go = (next: Tab) => navigate(next === "members" ? `/projects/${key}/team` : `/projects/${key}/team/${next}`);
  const changed = useCallback(() => void capabilities.reload(), [capabilities]);
  const handled = useCallback(() => setInviteRequested(false), []);
  return (
    <Tabs
      label="Équipe et accès"
      value={tab}
      onChange={go}
      items={tabs.map((item) => ({ ...item, count: item.id === "members" && directory.canAdminister ? directory.members.length : item.id === "teams" ? directory.teams.filter((team) => team.state === "active").length : undefined }))}
    >
      {tab === "members" && (
        <MembersPanel
          directory={directory}
          administration={project.administration}
          projectName={project.name}
          self={self}
          onChanged={changed}
          onInvite={() => {
            setInviteRequested(true);
            go("invitations");
          }}
        />
      )}
      {tab === "invitations" && (
        <InvitationsPanel
          scope={scope}
          directory={directory}
          administration={project.administration}
          projectName={project.name}
          self={self}
          inviteRequested={inviteRequested}
          onInviteHandled={handled}
        />
      )}
      {tab === "teams" && <TeamsPanel directory={directory} administration={project.administration} projectName={project.name} onChanged={changed} />}
    </Tabs>
  );
}
