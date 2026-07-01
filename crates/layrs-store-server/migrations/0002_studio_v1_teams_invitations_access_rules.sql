-- Studio V1 schema additions for team purposes and invitation team assignments.
-- Kept idempotent so existing dev databases created from 0001 can migrate in place.

ALTER TABLE teams
    ADD COLUMN IF NOT EXISTS purpose TEXT NOT NULL DEFAULT '';

ALTER TABLE invitations
    ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'pending';

ALTER TABLE invitations
    ADD COLUMN IF NOT EXISTS declined_at TIMESTAMPTZ;

CREATE TABLE IF NOT EXISTS invitation_team_assignments (
    invitation_id TEXT NOT NULL REFERENCES invitations(invitation_id) ON DELETE CASCADE,
    team_id TEXT NOT NULL REFERENCES teams(team_id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('maintainer', 'member')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (invitation_id, team_id)
);

CREATE INDEX IF NOT EXISTS invitations_workspace_status_created_idx
    ON invitations (workspace_id, status, created_at DESC);

CREATE INDEX IF NOT EXISTS invitations_email_status_idx
    ON invitations (lower(email), status);

CREATE INDEX IF NOT EXISTS invitation_team_assignments_team_idx
    ON invitation_team_assignments (team_id);
