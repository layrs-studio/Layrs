import type { FormEvent } from "react";
import type { Invitation, Team, TeamMember, TeamMemberRole } from "@layrs/client-sdk";
import { EmptyState, PanelTitle } from "./common";

export function TeamMembersPanel({
  invitations,
  members,
  mode = "all",
  onAddMember,
  onCreateInvitation,
  onRemoveMember,
  team
}: {
  invitations: Invitation[];
  members: TeamMember[];
  mode?: "all" | "members" | "invitations";
  onAddMember: (input: { email: string; role: TeamMemberRole }) => Promise<void>;
  onCreateInvitation: (email: string) => Promise<void>;
  onRemoveMember: (accountId: string) => Promise<void>;
  team: Team;
}) {
  async function submitMember(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    await onAddMember({
      email: String(form.get("email") ?? ""),
      role: String(form.get("role") ?? "member") as TeamMemberRole
    });
    event.currentTarget.reset();
  }

  async function submitInvitation(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    await onCreateInvitation(String(form.get("email") ?? ""));
    event.currentTarget.reset();
  }

  return (
    <section className="studio-panel studio-panel--wide" id="members">
      <PanelTitle
        eyebrow={mode === "invitations" ? "Invitations" : "Members"}
        title={mode === "invitations" ? "Pending access" : `${team.name} access`}
      />

      {mode !== "invitations" ? (
        <div className="studio-members-layout">
          <div className="studio-member-list" aria-label="Team members">
            {members.length === 0 ? (
              <EmptyState title="No members" detail="No active member records are linked to this Team yet." />
            ) : (
              members.map((member) => (
                <article className="studio-row" key={member.id}>
                  <div>
                    <strong>{member.name}</strong>
                    <p>{member.email}</p>
                  </div>
                  <div className="studio-row-actions">
                    <span>{member.role}</span>
                    <button type="button" onClick={() => void onRemoveMember(member.accountId)}>
                      Remove
                    </button>
                  </div>
                </article>
              ))
            )}
          </div>

          <form className="studio-form studio-member-form" onSubmit={submitMember}>
            <label className="studio-field">
              <span>Email</span>
              <input name="email" required type="email" />
            </label>
            <label className="studio-field">
              <span>Role</span>
              <select name="role" defaultValue="member">
                <option value="maintainer">maintainer</option>
                <option value="member">member</option>
              </select>
            </label>
            <button className="studio-primary-button" type="submit">
              Add member
            </button>
          </form>
        </div>
      ) : null}

      {mode !== "members" ? (
      <section className="studio-subsection" aria-label="Pending invitations">
        <h3>Pending invitations</h3>
        {invitations.length === 0 ? (
          <EmptyState title="No pending invitations" detail="No pending invitations are linked to this Team." />
        ) : (
          <div className="studio-list">
            {invitations.map((invitation) => (
              <article className="studio-row" key={invitation.id}>
                <div>
                  <strong>{invitation.email}</strong>
                  <p>Workspace role: {invitation.role}</p>
                </div>
                <span>{invitation.status}</span>
              </article>
            ))}
          </div>
        )}
        <form className="studio-form studio-inline-form" onSubmit={submitInvitation}>
          <label className="studio-field">
            <span>Email</span>
            <input name="email" required type="email" />
          </label>
          <button className="studio-primary-button" type="submit">
            Create invitation
          </button>
        </form>
      </section>
      ) : null}
    </section>
  );
}
