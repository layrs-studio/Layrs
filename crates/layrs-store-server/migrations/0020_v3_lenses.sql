-- Installed Lens packets are immutable. Retirement changes visibility, never
-- the packet bytes: pinned Spaces can continue to fetch the exact version.
CREATE TABLE layrs_collaboration.lens_versions (
    deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id),
    lens_id TEXT NOT NULL,
    version TEXT NOT NULL,
    digest TEXT NOT NULL CHECK (digest ~ '^sha256:[0-9a-f]{64}$'),
    manifest_jcs TEXT NOT NULL CHECK (octet_length(manifest_jcs) BETWEEN 1 AND 262144),
    packet_jcs TEXT NOT NULL CHECK (octet_length(packet_jcs) BETWEEN 1 AND 12582912),
    origin TEXT NOT NULL CHECK (origin IN ('builtin','installed')),
    state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active','retired','revoked')),
    installed_by TEXT REFERENCES principals(principal_id),
    installed_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (deployment_id,lens_id,version,digest),
    UNIQUE (deployment_id,lens_id,version)
);

-- Only visibility may change. No writer can replace bytes behind a selected pin.
CREATE FUNCTION layrs_collaboration.guard_lens_version_update() RETURNS trigger
LANGUAGE plpgsql AS $lens_guard$
BEGIN
    IF (NEW.deployment_id,NEW.lens_id,NEW.version,NEW.digest,NEW.manifest_jcs,NEW.packet_jcs,NEW.origin,NEW.installed_by,NEW.installed_at)
       IS DISTINCT FROM
       (OLD.deployment_id,OLD.lens_id,OLD.version,OLD.digest,OLD.manifest_jcs,OLD.packet_jcs,OLD.origin,OLD.installed_by,OLD.installed_at)
       OR (OLD.state='retired' AND NEW.state='active')
       OR (OLD.state='revoked' AND NEW.state<>'revoked') THEN
        RAISE EXCEPTION 'immutable Lens packet or invalid state transition';
    END IF;
    RETURN NEW;
END $lens_guard$;
CREATE TRIGGER guard_lens_version_update BEFORE UPDATE ON layrs_collaboration.lens_versions
    FOR EACH ROW EXECUTE FUNCTION layrs_collaboration.guard_lens_version_update();

CREATE TABLE layrs_collaboration.lens_state_events (
    event_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    deployment_id TEXT NOT NULL,
    lens_id TEXT NOT NULL,
    version TEXT NOT NULL,
    digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('retired','revoked')),
    actor_principal_id TEXT NOT NULL REFERENCES principals(principal_id),
    changed_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    FOREIGN KEY (deployment_id,lens_id,version,digest)
      REFERENCES layrs_collaboration.lens_versions(deployment_id,lens_id,version,digest)
);

CREATE TABLE layrs_collaboration.space_lens_configs (
    space_id TEXT PRIMARY KEY REFERENCES spaces(space_id),
    revision BIGINT NOT NULL CHECK (revision BETWEEN 1 AND 9007199254740991),
    pins_jcs TEXT NOT NULL CHECK (octet_length(pins_jcs) <= 65536),
    associations_jcs TEXT NOT NULL CHECK (octet_length(associations_jcs) <= 65536),
    updated_by TEXT REFERENCES principals(principal_id),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp()
);

CREATE TABLE layrs_collaboration.space_lens_pins (
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id),
    lens_id TEXT NOT NULL,
    version TEXT NOT NULL,
    digest TEXT NOT NULL,
    PRIMARY KEY (space_id,lens_id),
    FOREIGN KEY (deployment_id,lens_id,version,digest)
        REFERENCES layrs_collaboration.lens_versions(deployment_id,lens_id,version,digest)
);
CREATE INDEX space_lens_pins_version ON layrs_collaboration.space_lens_pins(lens_id,version,digest);

-- Retains proof that a package was selected here, even after unpin/revocation.
CREATE TABLE layrs_collaboration.space_lens_history (
    space_id TEXT NOT NULL REFERENCES spaces(space_id),
    deployment_id TEXT NOT NULL REFERENCES deployments(deployment_id),
    lens_id TEXT NOT NULL,
    version TEXT NOT NULL,
    digest TEXT NOT NULL,
    first_selected_at TIMESTAMPTZ NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (space_id,lens_id,version,digest),
    FOREIGN KEY (deployment_id,lens_id,version,digest)
        REFERENCES layrs_collaboration.lens_versions(deployment_id,lens_id,version,digest)
);

DO $lens_grants$
DECLARE writer TEXT;
BEGIN
    SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1;
    EXECUTE format('GRANT SELECT,INSERT,UPDATE ON layrs_collaboration.lens_versions,layrs_collaboration.space_lens_configs TO %I',writer);
    -- SELECT FOR SHARE during authorized packet delivery/execution requires UPDATE privilege.
    EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON layrs_collaboration.space_lens_pins TO %I',writer);
    EXECUTE format('GRANT SELECT,INSERT ON layrs_collaboration.space_lens_history TO %I',writer);
    EXECUTE format('GRANT SELECT,INSERT ON layrs_collaboration.lens_state_events TO %I',writer);
    EXECUTE format('GRANT USAGE,SELECT ON SEQUENCE layrs_collaboration.lens_state_events_event_id_seq TO %I',writer);
END
$lens_grants$;
