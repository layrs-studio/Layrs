-- Nominative invitations; no bearer secret or second authorization authority.
CREATE TABLE layrs_collaboration.space_invitations (
    invitation_id TEXT PRIMARY KEY CHECK (length(invitation_id) BETWEEN 16 AND 128),
    sequence BIGSERIAL UNIQUE,
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    inviter_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    recipient_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    member_role TEXT NOT NULL CHECK (member_role IN ('member','auditor')),
    request_digest TEXT NOT NULL,
    policy_set_id TEXT NOT NULL REFERENCES policy_sets(policy_set_id),
    authorization_epoch BIGINT NOT NULL CHECK (authorization_epoch >= 0),
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending','accepted','rejected','revoked')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL,
    decided_at TIMESTAMPTZ,
    proposal_object_id TEXT NOT NULL REFERENCES canonical_objects(object_id),
    decision_object_id TEXT REFERENCES canonical_objects(object_id),
    grant_result_jcs TEXT CHECK (octet_length(grant_result_jcs) <= 4096),
    CHECK (inviter_principal_id <> recipient_principal_id),
    CHECK (expires_at > created_at),
    CHECK ((state='pending' AND decided_at IS NULL AND decision_object_id IS NULL)
        OR (state<>'pending' AND decided_at IS NOT NULL AND decision_object_id IS NOT NULL)),
    CHECK ((state='accepted') = (grant_result_jcs IS NOT NULL))
);
CREATE INDEX space_invitations_recipient ON layrs_collaboration.space_invitations(recipient_principal_id,sequence);
CREATE INDEX space_invitations_space ON layrs_collaboration.space_invitations(space_id,sequence);
DO $invitation_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT,INSERT ON layrs_collaboration.space_invitations TO %I',writer);
    EXECUTE format('GRANT UPDATE(state,decided_at,decision_object_id,grant_result_jcs) ON layrs_collaboration.space_invitations TO %I',writer);
    EXECUTE format('GRANT USAGE,SELECT ON SEQUENCE layrs_collaboration.space_invitations_sequence_seq TO %I',writer);
END
$invitation_grants$;
