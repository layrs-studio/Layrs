import { useState, type FormEvent } from "react";
import type { Space, StudioSnapshot, Team } from "@layrs/client-sdk";
import { ActionGroup, StatusPill } from "@layrs/ui";
import { spaceHref, teamHref } from "../routes";
import { EmptyState, Metric, PanelTitle, TextField } from "../components/common";

type CreateDialog = "space" | "team" | null;

export function HomePage({
  activeSpaceId,
  favoriteSpaceIds,
  onCreateSpace,
  onCreateTeam,
  onNavigate,
  onToggleFavorite,
  snapshot
}: {
  activeSpaceId?: string;
  favoriteSpaceIds: string[];
  onCreateSpace: (input: { name: string; key: string; teamId?: string; description?: string }) => Promise<void>;
  onCreateTeam: (input: { name: string }) => Promise<void>;
  onNavigate: (href: string) => void;
  onToggleFavorite: (spaceId: string) => void;
  snapshot: StudioSnapshot;
}) {
  const [dialog, setDialog] = useState<CreateDialog>(null);

  async function createTeam(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    await onCreateTeam({ name: String(form.get("name") ?? "") });
    event.currentTarget.reset();
    setDialog(null);
  }

  async function createSpace(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    await onCreateSpace({
      name: String(form.get("name") ?? ""),
      key: String(form.get("key") ?? ""),
      teamId: String(form.get("teamId") ?? "") || undefined,
      description: String(form.get("description") ?? "") || undefined
    });
    event.currentTarget.reset();
    setDialog(null);
  }

  return (
    <section className="studio-grid" aria-label="Workspace">
      <section className="studio-panel studio-panel--wide" id="workspace">
        <div className="studio-page-heading">
          <PanelTitle eyebrow="Workspace" title={snapshot.workspace.name} />
          <ActionGroup>
            <button className="studio-secondary-button" type="button" onClick={() => setDialog("team")}>
              Create Team
            </button>
            <button className="studio-primary-button" type="button" onClick={() => setDialog("space")}>
              Create Space
            </button>
          </ActionGroup>
        </div>
        <div className="studio-workspace-summary">
          <p>{snapshot.workspace.description || "Server-backed Layrs workspace."}</p>
          <div className="studio-metrics" aria-label="Workspace metrics">
            <Metric label="Teams" value={snapshot.teams.length} />
            <Metric label="Spaces" value={snapshot.spaces.length} />
            <Metric label="Layers" value={snapshot.layers.length} />
            <Metric label="Audit" value={snapshot.auditEvents.length} />
          </div>
        </div>
      </section>

      <section className="studio-panel" id="spaces">
        <PanelTitle eyebrow="Spaces" title="Workspace Spaces" />
        {snapshot.spaces.length === 0 ? (
          <EmptyState title="No Spaces" detail="Create a Space to start organizing Layers and files." />
        ) : (
          <SpaceList
            activeSpaceId={activeSpaceId}
            favoriteSpaceIds={favoriteSpaceIds}
            onNavigate={onNavigate}
            onToggleFavorite={onToggleFavorite}
            spaces={snapshot.spaces}
            teams={snapshot.teams}
          />
        )}
      </section>

      <section className="studio-panel" id="teams">
        <PanelTitle eyebrow="Teams" title="Workspace Teams" />
        {snapshot.teams.length === 0 ? (
          <EmptyState title="No Teams" detail="Create a Team before assigning Space ownership." />
        ) : (
          <TeamList onNavigate={onNavigate} teams={snapshot.teams} />
        )}
      </section>

      {dialog ? (
        <div className="studio-modal-backdrop" role="presentation">
          <section className="studio-modal" role="dialog" aria-modal="true" aria-labelledby="studio-create-title">
            <div className="studio-page-heading">
              <PanelTitle eyebrow="Create" title={dialog === "space" ? "New Space" : "New Team"} />
              <button className="studio-ghost-button" type="button" onClick={() => setDialog(null)}>
                Close
              </button>
            </div>
            {dialog === "space" ? (
              <form className="studio-form" onSubmit={createSpace}>
                <TextField label="Name" name="name" required />
                <TextField label="Key" name="key" pattern="[a-z0-9-]+" required />
                <TextField label="Description" name="description" />
                <label className="studio-field">
                  <span>Team</span>
                  <select name="teamId" defaultValue={snapshot.teams[0]?.id ?? ""}>
                    {snapshot.teams.length === 0 ? <option value="">No team</option> : null}
                    {snapshot.teams.map((team) => (
                      <option key={team.id} value={team.id}>
                        {team.name}
                      </option>
                    ))}
                  </select>
                </label>
                <button className="studio-primary-button" type="submit">
                  Create Space
                </button>
              </form>
            ) : (
              <form className="studio-form" onSubmit={createTeam}>
                <TextField label="Name" name="name" required />
                <button className="studio-primary-button" type="submit">
                  Create Team
                </button>
              </form>
            )}
          </section>
        </div>
      ) : null}
    </section>
  );
}

function TeamList({ onNavigate, teams }: { onNavigate: (href: string) => void; teams: Team[] }) {
  return (
    <div className="studio-list">
      {teams.map((team) => (
        <a
          className="studio-row studio-row-link"
          href={teamHref(team.id)}
          key={team.id}
          onClick={(event) => {
            event.preventDefault();
            onNavigate(teamHref(team.id));
          }}
        >
          <div>
            <strong>{team.name}</strong>
            <p>{team.purpose}</p>
          </div>
          <span>{team.members} members</span>
        </a>
      ))}
    </div>
  );
}

function SpaceList({
  activeSpaceId,
  favoriteSpaceIds,
  onNavigate,
  onToggleFavorite,
  spaces,
  teams
}: {
  activeSpaceId?: string;
  favoriteSpaceIds: string[];
  onNavigate: (href: string) => void;
  onToggleFavorite: (spaceId: string) => void;
  spaces: Space[];
  teams: Team[];
}) {
  return (
    <div className="studio-list">
      {spaces.map((space) => {
        const team = teams.find((item) => item.id === space.teamId);
        return (
          <article
            className={space.id === activeSpaceId ? "studio-row studio-row-with-actions is-active" : "studio-row studio-row-with-actions"}
            key={space.id}
          >
            <button className="studio-row-main-button" onClick={() => onNavigate(spaceHref(space.id))} type="button">
              <div>
                <strong>{space.name}</strong>
                <p>{space.description}</p>
                <small>{team?.name ?? "Unassigned Team"}</small>
              </div>
              <StatusPill status={space.status} />
            </button>
            <div className="studio-row-actions">
              <button
                aria-pressed={favoriteSpaceIds.includes(space.id)}
                className={favoriteSpaceIds.includes(space.id) ? "studio-star-button is-active" : "studio-star-button"}
                onClick={() => onToggleFavorite(space.id)}
                title={favoriteSpaceIds.includes(space.id) ? "Remove from sidebar favorites" : "Add to sidebar favorites"}
                type="button"
              >
                {favoriteSpaceIds.includes(space.id) ? "Favorited" : "Favorite"}
              </button>
            </div>
          </article>
        );
      })}
    </div>
  );
}
