-- Add authenticated client assertions without changing canonical Step identity
-- or rewriting any previous server-admission observation.
ALTER TABLE layrs_collaboration.published_step_observations
    ADD COLUMN provenance TEXT NOT NULL DEFAULT 'server_admission'
    CHECK (provenance IN ('server_admission','client_assertion'));
ALTER TABLE layrs_collaboration.published_step_observations
    DROP CONSTRAINT published_step_observations_space_id_step_id_key;
CREATE UNIQUE INDEX published_server_admission_once
    ON layrs_collaboration.published_step_observations(space_id,step_id)
    WHERE provenance='server_admission';
CREATE INDEX published_client_observation_budget
    ON layrs_collaboration.published_step_observations(space_id,principal_id)
    WHERE provenance='client_assertion';
