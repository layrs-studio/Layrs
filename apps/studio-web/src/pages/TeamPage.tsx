import { useState } from "react";
import type { Invitation, Space, Team, TeamMember, TeamMemberRole } from "@layrs/client-sdk";
import { StatusPill, Tabs } from "@layrs/ui";
import { spaceHref } from "../routes";
import { EmptyState, PanelTitle } from "../components/common";
import { TeamMembersPanel } from "../components/TeamMembersPanel";

type TeamTab = "members" | "invitations" | "spaces" | "settings";

export function TeamPage({
  invitations,
  members,
  onAddMember,
  onCreateInvitation,
  onNavigate,
  onRemoveMember,
  spaces,
  team
}: {
  invitations: Invitation[];
  members: TeamMember[];
  onAddMember: (input: { email: string; role: TeamMemberRole }) => Promise<void>;
  onCreateInvitation: (email: string) => Promise<void>;
  onNavigate: (href: string) => void;
  onRemoveMember: (accountId: string) => Promise<void>;
  spaces: Space[];
  team?: Team;
}) {
  const [activeTab, setActiveTab] = useState<TeamTab>("members");

  if (!team) {
    return <EmptyState title="Team not found" detail="Choose an existing Team from the Workspace page." />;
  }

  const ownedSpaces = spaces.filter((space) => space.teamId === team.id);

  return (
    <section className="studio-grid" aria-label="Team">
      <section className="studio-panel studio-panel--wide">
        <div className="studio-page-heading">
          <PanelTitle eyebrow="Team" title={team.name} />
          <StatusPill status="passing" label={`${members.length} members`} />
        </div>
        <p className="studio-muted">{team.purpose}</p>
      </section>

      <section className="studio-panel studio-panel--wide" aria-label="Team tabs">
        <Tabs
          activeId={activeTab}
          ariaLabel="Team sections"
          onChange={(nextTab) => setActiveTab(nextTab as TeamTab)}
          tabs={[
            { id: "members", label: "Members", count: members.length },
            { id: "invitations", label: "Invitations", count: invitations.length },
            { id: "spaces", label: "Spaces", count: ownedSpaces.length },
            { id: "settings", label: "Settings" }
          ]}
        />
      </section>

      {activeTab === "members" ? (
        <TeamMembersPanel
          invitations={invitations}
          members={members}
          mode="members"
          onAddMember={onAddMember}
          onCreateInvitation={onCreateInvitation}
          onRemoveMember={onRemoveMember}
          team={team}
        />
      ) : null}

      {activeTab === "invitations" ? (
        <TeamMembersPanel
          invitations={invitations}
          members={members}
          mode="invitations"
          onAddMember={onAddMember}
          onCreateInvitation={onCreateInvitation}
          onRemoveMember={onRemoveMember}
          team={team}
        />
      ) : null}

      {activeTab === "spaces" ? (
        <section className="studio-panel studio-panel--wide" id="team-spaces">
          <PanelTitle eyebrow="Spaces" title="Owned Spaces" />
          {ownedSpaces.length === 0 ? (
            <EmptyState title="No Spaces" detail="This Team is not assigned to any Space yet." />
          ) : (
            <div className="studio-list">
              {ownedSpaces.map((space) => (
                <a
                  className="studio-row studio-row-link"
                  href={spaceHref(space.id)}
                  key={space.id}
                  onClick={(event) => {
                    event.preventDefault();
                    onNavigate(spaceHref(space.id));
                  }}
                >
                  <div>
                    <strong>{space.name}</strong>
                    <p>{space.description}</p>
                  </div>
                  <StatusPill status={space.status} />
                </a>
              ))}
            </div>
          )}
        </section>
      ) : null}

      {activeTab === "settings" ? (
        <section className="studio-panel studio-panel--wide">
          <PanelTitle eyebrow="Settings" title="Team administration" />
          <div className="studio-settings-grid">
            <div className="studio-setting-card">
              <span>Purpose</span>
              <strong>{team.purpose || "General access"}</strong>
              <p>Team-level settings will host ownership, default Space access and future gate responsibilities.</p>
            </div>
            <div className="studio-setting-card">
              <span>Linked Spaces</span>
              <strong>{ownedSpaces.length}</strong>
              <p>Use the Spaces tab to inspect projects owned by this Team.</p>
            </div>
          </div>
        </section>
      ) : null}
    </section>
  );
}
