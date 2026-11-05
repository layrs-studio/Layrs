\set ON_ERROR_STOP on

BEGIN;
SET CONSTRAINTS ALL DEFERRED;

INSERT INTO stores (store_id, backend_kind, backend_config_jcs)
VALUES ('store:test', 'filesystem', '{}');
INSERT INTO deployments (deployment_id, store_id, display_name)
VALUES ('deployment:test', 'store:test', 'test');
INSERT INTO principals (
    principal_id, actor_id, deployment_id, principal_kind, subject_digest, display_name
) VALUES
    ('principal:alice', 'actor:alice', 'deployment:test', 'account', 'subject:alice', 'Alice'),
    ('principal:bob', 'actor:bob', 'deployment:test', 'account', 'subject:bob', 'Bob');
INSERT INTO workspaces (
    workspace_id, deployment_id, slug, display_name, created_by_principal_id
) VALUES ('workspace:test', 'deployment:test', 'test', 'Test', 'principal:alice');
INSERT INTO workspace_memberships (workspace_id, principal_id, role)
VALUES
    ('workspace:test', 'principal:alice', 'owner'),
    ('workspace:test', 'principal:bob', 'member');

INSERT INTO spaces (
    space_id, workspace_id, slug, display_name, lifecycle_state,
    current_repository_view_id, current_repository_view_generation,
    current_policy_set_id, authorization_epoch, created_by_principal_id
) VALUES
    (
        'space:a', 'workspace:test', 'a', 'A', 'ready',
        'blake3:1111111111111111111111111111111111111111111111111111111111111111', 0,
        'blake3:2222222222222222222222222222222222222222222222222222222222222222', 1,
        'principal:alice'
    ),
    (
        'space:b', 'workspace:test', 'b', 'B', 'ready',
        'blake3:3333333333333333333333333333333333333333333333333333333333333333', 0,
        'blake3:4444444444444444444444444444444444444444444444444444444444444444', 1,
        'principal:alice'
    );

INSERT INTO canonical_objects (
    object_id, store_id, object_kind, canonical_byte_length, encoded_byte_length,
    compression, storage_key, verification_state, verified_at
) VALUES
    ('blake3:1111111111111111111111111111111111111111111111111111111111111111', 'store:test', 'repository_view', 2, 2, 'identity', '1', 'verified', now()),
    ('blake3:2222222222222222222222222222222222222222222222222222222222222222', 'store:test', 'policy_set', 2, 2, 'identity', '2', 'verified', now()),
    ('blake3:3333333333333333333333333333333333333333333333333333333333333333', 'store:test', 'repository_view', 2, 2, 'identity', '3', 'verified', now()),
    ('blake3:4444444444444444444444444444444444444444444444444444444444444444', 'store:test', 'policy_set', 2, 2, 'identity', '4', 'verified', now()),
    ('blake3:5555555555555555555555555555555555555555555555555555555555555555', 'store:test', 'product', 2, 2, 'identity', '5', 'verified', now()),
    ('blake3:6666666666666666666666666666666666666666666666666666666666666666', 'store:test', 'product', 2, 2, 'identity', '6', 'verified', now()),
    ('blake3:7777777777777777777777777777777777777777777777777777777777777777', 'store:test', 'product', 2, 2, 'identity', '7', 'verified', now()),
    ('blake3:8888888888888888888888888888888888888888888888888888888888888888', 'store:test', 'tree', 2, 2, 'identity', '8', 'verified', now()),
    ('blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee', 'store:test', 'file_manifest', 2, 2, 'identity', 'e', 'verified', now()),
    ('blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff', 'store:test', 'chunk', 2, 2, 'identity', 'f', 'verified', now()),
    ('blake3:0000000000000000000000000000000000000000000000000000000000000000', 'store:test', 'chunk', 2, 2, 'identity', '0', 'verified', now()),
    ('blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'store:test', 'product', 2, 2, 'identity', 'a', 'verified', now()),
    ('blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'store:test', 'product', 2, 2, 'identity', 'b', 'verified', now()),
    ('blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc', 'store:test', 'product', 2, 2, 'identity', 'c', 'verified', now()),
    ('blake3:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd', 'store:test', 'product', 2, 2, 'identity', 'd-product', 'verified', now()),
    ('blake3:1212121212121212121212121212121212121212121212121212121212121212', 'store:test', 'authorization_snapshot', 2, 2, 'identity', 'authorization-snapshot-1', 'verified', now());
INSERT INTO chunks (chunk_id, raw_byte_length)
VALUES
    ('blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff', 2),
    ('blake3:0000000000000000000000000000000000000000000000000000000000000000', 2);
INSERT INTO file_manifests (
    file_manifest_id, file_id, file_byte_length, chunker_name, chunker_version,
    chunker_parameters_jcs, reconstruction_verified_at
) VALUES (
    'blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
    'blake3:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',
    4, 'fixed', '1', '{}', now()
);
INSERT INTO file_manifest_chunks (
    file_manifest_id, ordinal, chunk_id, byte_offset, byte_length
) VALUES
    (
        'blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
        0, 'blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff', 0, 2
    ),
    (
        'blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
        1, 'blake3:0000000000000000000000000000000000000000000000000000000000000000', 2, 2
    );

INSERT INTO policy_sets (policy_set_id, space_id, canonical_record_jcs)
VALUES
    ('blake3:2222222222222222222222222222222222222222222222222222222222222222', 'space:a', '{}'),
    ('blake3:4444444444444444444444444444444444444444444444444444444444444444', 'space:b', '{}');
INSERT INTO authorization_snapshots (
    authorization_snapshot_id, space_id, principal_id, policy_set_id,
    authorization_epoch, request_binding_jcs, decisions_jcs,
    canonical_record_jcs, evaluated_at
) VALUES (
    'blake3:1212121212121212121212121212121212121212121212121212121212121212',
    'space:a', 'principal:alice',
    'blake3:2222222222222222222222222222222222222222222222222222222222222222',
    1, '{}', '{}', '{}', now()
);
INSERT INTO repository_views (repository_view_id, space_id, canonical_record_jcs)
VALUES
    ('blake3:1111111111111111111111111111111111111111111111111111111111111111', 'space:a', '{}'),
    ('blake3:3333333333333333333333333333333333333333333333333333333333333333', 'space:b', '{}');
INSERT INTO products (
    product_object_id, space_id, product_kind, logical_id, canonical_envelope_jcs
) VALUES
    ('blake3:5555555555555555555555555555555555555555555555555555555555555555', 'space:a', 'marker', 'marker:m1', '{}'),
    ('blake3:6666666666666666666666666666666666666666666666666666666666666666', 'space:a', 'release', 'release:r1', '{}'),
    ('blake3:7777777777777777777777777777777777777777777777777777777777777777', 'space:b', 'marker', 'marker:m2', '{}'),
    ('blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'space:a', 'layer', 'layer:l1', '{}'),
    ('blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'space:b', 'layer', 'layer:l1', '{}');
INSERT INTO products (
    product_object_id, space_id, product_kind, logical_id,
    previous_product_object_id, canonical_envelope_jcs
) VALUES (
    'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
    'space:a', 'layer', 'layer:l1',
    'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', '{}'
);
INSERT INTO products (
    product_object_id, space_id, product_kind, logical_id,
    previous_product_object_id, canonical_envelope_jcs
) VALUES (
    'blake3:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',
    'space:a', 'layer', 'layer:l1',
    'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc', '{}'
);

COMMIT;

DO $principal_actor_mapping$
DECLARE rejected BOOLEAN;
BEGIN
    rejected := false;
    BEGIN
        INSERT INTO principals (
            principal_id, actor_id, deployment_id, principal_kind, subject_digest, display_name
        ) VALUES (
            'principal:invalid-actor', 'principal:not-an-actor', 'deployment:test',
            'account', 'subject:invalid-actor', 'Invalid actor'
        );
    EXCEPTION WHEN check_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'Principal accepted an invalid ActorId'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO principals (
            principal_id, actor_id, deployment_id, principal_kind, subject_digest, display_name
        ) VALUES (
            'principal:duplicate-actor', 'actor:alice', 'deployment:test',
            'account', 'subject:duplicate-actor', 'Duplicate actor'
        );
    EXCEPTION WHEN unique_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'ActorId mapped to more than one PrincipalId'; END IF;
END
$principal_actor_mapping$;

INSERT INTO sync_transactions (
    transaction_id, space_id, client_id, idempotency_key, request_digest,
    principal_id, authorization_snapshot_id, authorization_epoch,
    resulting_authorization_epoch, receipt_canonical_jcs, receipt_digest,
    first_event_sequence, last_event_sequence
) VALUES (
    'transaction:event-test', 'space:a', 'client:test', 'event-test-key-0001',
    'blake3:1212121212121212121212121212121212121212121212121212121212121212',
    'principal:alice',
    'blake3:1212121212121212121212121212121212121212121212121212121212121212',
    1, 1, '{}',
    'blake3:3434343434343434343434343434343434343434343434343434343434343434',
    1, 1
);

DO $event_actor_rejections$
DECLARE rejected BOOLEAN;
BEGIN
    rejected := false;
    BEGIN
        INSERT INTO space_events (
            space_id, sequence, event_id, transaction_id, event_kind,
            actor_kind, actor_id, device_id, payload_canonical_jcs
        ) VALUES (
            'space:a', 2, 'event:invalid-kind', 'transaction:event-test', 'steps.accepted',
            'human', 'actor:alice', NULL, '{}'
        );
    EXCEPTION WHEN check_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'Event accepted an invalid actor kind'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO space_events (
            space_id, sequence, event_id, transaction_id, event_kind,
            actor_kind, actor_id, device_id, payload_canonical_jcs
        ) VALUES (
            'space:a', 2, 'event:invalid-actor', 'transaction:event-test', 'steps.accepted',
            'account', 'principal:alice', NULL, '{}'
        );
    EXCEPTION WHEN check_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'Event accepted an invalid ActorId'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO space_events (
            space_id, sequence, event_id, transaction_id, event_kind,
            actor_kind, actor_id, device_id, payload_canonical_jcs
        ) VALUES (
            'space:a', 2, 'event:unknown-actor', 'transaction:event-test', 'steps.accepted',
            'account', 'actor:unknown', NULL, '{}'
        );
    EXCEPTION WHEN foreign_key_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'Event accepted an unmapped ActorId'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO space_events (
            space_id, sequence, event_id, transaction_id, event_kind,
            actor_kind, actor_id, device_id, payload_canonical_jcs
        ) VALUES (
            'space:a', 2, 'event:invalid-device', 'transaction:event-test', 'steps.accepted',
            'account', 'actor:alice', 'client:not-a-device', '{}'
        );
    EXCEPTION WHEN check_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'Event accepted an invalid DeviceId'; END IF;
END
$event_actor_rejections$;

INSERT INTO space_events (
    space_id, sequence, event_id, transaction_id, event_kind,
    actor_kind, actor_id, device_id, payload_canonical_jcs
) VALUES (
    'space:a', 1, 'event:actor-exact', 'transaction:event-test', 'steps.accepted',
    'account', 'actor:alice', 'device:desktop-a', '{}'
);
DO $event_actor_exact_replay$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM space_events
        WHERE space_id = 'space:a' AND sequence = 1
          AND actor_kind = 'account'
          AND actor_id = 'actor:alice'
          AND device_id = 'device:desktop-a'
    ) THEN
        RAISE EXCEPTION 'EventActor did not replay exactly from durable columns';
    END IF;
END
$event_actor_exact_replay$;

INSERT INTO repository_view_markers (
    space_id, repository_view_id, marker_id, marker_product_object_id
) VALUES (
    'space:a',
    'blake3:1111111111111111111111111111111111111111111111111111111111111111',
    'marker:m1',
    'blake3:5555555555555555555555555555555555555555555555555555555555555555'
);
INSERT INTO repository_view_releases (
    space_id, repository_view_id, release_id, release_product_object_id
) VALUES (
    'space:a',
    'blake3:1111111111111111111111111111111111111111111111111111111111111111',
    'release:r1',
    'blake3:6666666666666666666666666666666666666666666666666666666666666666'
);

DO $typed_products$
DECLARE
    rejected BOOLEAN;
BEGIN
    rejected := false;
    BEGIN
        INSERT INTO repository_view_markers (
            space_id, repository_view_id, marker_id, marker_product_object_id
        ) VALUES (
            'space:a',
            'blake3:1111111111111111111111111111111111111111111111111111111111111111',
            'marker:missing',
            'blake3:9999999999999999999999999999999999999999999999999999999999999999'
        );
        SET CONSTRAINTS ALL IMMEDIATE;
    EXCEPTION WHEN foreign_key_violation THEN
        rejected := true;
    END;
    SET CONSTRAINTS ALL DEFERRED;
    IF NOT rejected THEN RAISE EXCEPTION 'missing Marker Product was accepted'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO repository_view_markers (
            space_id, repository_view_id, marker_id, marker_product_object_id
        ) VALUES (
            'space:a',
            'blake3:1111111111111111111111111111111111111111111111111111111111111111',
            'marker:m2',
            'blake3:7777777777777777777777777777777777777777777777777777777777777777'
        );
        SET CONSTRAINTS ALL IMMEDIATE;
    EXCEPTION WHEN foreign_key_violation THEN
        rejected := true;
    END;
    SET CONSTRAINTS ALL DEFERRED;
    IF NOT rejected THEN RAISE EXCEPTION 'cross-Space Marker Product was accepted'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO repository_view_markers (
            space_id, repository_view_id, marker_id, marker_product_object_id
        ) VALUES (
            'space:a',
            'blake3:1111111111111111111111111111111111111111111111111111111111111111',
            'marker:r1',
            'blake3:6666666666666666666666666666666666666666666666666666666666666666'
        );
        SET CONSTRAINTS ALL IMMEDIATE;
    EXCEPTION WHEN foreign_key_violation THEN
        rejected := true;
    END;
    SET CONSTRAINTS ALL DEFERRED;
    IF NOT rejected THEN RAISE EXCEPTION 'wrong-kind Marker Product was accepted'; END IF;
END
$typed_products$;

DO $product_identity_boundaries$
DECLARE
    kind TEXT;
    logical_id TEXT;
    digest TEXT;
    first_object_id TEXT;
    second_object_id TEXT;
    rejected BOOLEAN;
BEGIN
    FOREACH kind IN ARRAY ARRAY[
        'marker', 'release', 'proof', 'gateEvaluation', 'weave', 'decision'
    ] LOOP
        logical_id := CASE kind
            WHEN 'marker' THEN 'marker:singleton-test'
            WHEN 'release' THEN 'release:singleton-test'
            WHEN 'proof' THEN 'proof:singleton-test'
            WHEN 'gateEvaluation' THEN 'gate-evaluation:singleton-test'
            WHEN 'weave' THEN 'weave:singleton-test'
            WHEN 'decision' THEN 'decision:singleton-test'
        END;
        digest := pg_catalog.md5('singleton:first:' || kind);
        first_object_id := 'blake3:' || digest || digest;
        INSERT INTO canonical_objects (
            object_id, store_id, object_kind, canonical_byte_length, encoded_byte_length,
            compression, storage_key, verification_state, verified_at
        ) VALUES (
            first_object_id, 'store:test', 'product', 2, 2,
            'identity', 'singleton:first:' || kind, 'verified', now()
        );
        INSERT INTO products (
            product_object_id, space_id, product_kind, logical_id, canonical_envelope_jcs
        ) VALUES (first_object_id, 'space:a', kind, logical_id, '{}');

        rejected := false;
        BEGIN
            digest := pg_catalog.md5('singleton:second:' || kind);
            second_object_id := 'blake3:' || digest || digest;
            INSERT INTO canonical_objects (
                object_id, store_id, object_kind, canonical_byte_length, encoded_byte_length,
                compression, storage_key, verification_state, verified_at
            ) VALUES (
                second_object_id, 'store:test', 'product', 2, 2,
                'identity', 'singleton:second:' || kind, 'verified', now()
            );
            INSERT INTO products (
                product_object_id, space_id, product_kind, logical_id, canonical_envelope_jcs
            ) VALUES (second_object_id, 'space:a', kind, logical_id, '{}');
        EXCEPTION WHEN unique_violation THEN rejected := true;
        END;
        IF NOT rejected THEN
            RAISE EXCEPTION 'singleton Product kind % accepted a duplicate logical identity', kind;
        END IF;
    END LOOP;

    FOREACH kind IN ARRAY ARRAY['gateDefinition', 'policy'] LOOP
        logical_id := CASE kind
            WHEN 'gateDefinition' THEN 'gate-definition:versioned-test'
            WHEN 'policy' THEN 'policy:versioned-test'
        END;
        digest := pg_catalog.md5('versioned:first:' || kind);
        first_object_id := 'blake3:' || digest || digest;
        INSERT INTO canonical_objects (
            object_id, store_id, object_kind, canonical_byte_length, encoded_byte_length,
            compression, storage_key, verification_state, verified_at
        ) VALUES (
            first_object_id, 'store:test', 'product', 2, 2,
            'identity', 'versioned:first:' || kind, 'verified', now()
        );
        INSERT INTO products (
            product_object_id, space_id, product_kind, logical_id, canonical_envelope_jcs
        ) VALUES (first_object_id, 'space:a', kind, logical_id, '{}');

        digest := pg_catalog.md5('versioned:second:' || kind);
        second_object_id := 'blake3:' || digest || digest;
        INSERT INTO canonical_objects (
            object_id, store_id, object_kind, canonical_byte_length, encoded_byte_length,
            compression, storage_key, verification_state, verified_at
        ) VALUES (
            second_object_id, 'store:test', 'product', 2, 2,
            'identity', 'versioned:second:' || kind, 'verified', now()
        );
        INSERT INTO products (
            product_object_id, space_id, product_kind, logical_id, canonical_envelope_jcs
        ) VALUES (second_object_id, 'space:a', kind, logical_id, '{}');
    END LOOP;

    IF (
        SELECT count(*) FROM products AS product
        WHERE product.space_id = 'space:a'
          AND product.product_kind = 'layer'
          AND product.logical_id = 'layer:l1'
    ) <> 3 THEN
        RAISE EXCEPTION 'Layer Product versions were collapsed';
    END IF;
    IF (
        SELECT count(*) FROM products AS product
        WHERE product.space_id = 'space:a'
          AND product.product_kind = 'gateDefinition'
          AND product.logical_id = 'gate-definition:versioned-test'
    ) <> 2 THEN
        RAISE EXCEPTION 'GateDefinition Product versions were collapsed';
    END IF;
    IF (
        SELECT count(*) FROM products AS product
        WHERE product.space_id = 'space:a'
          AND product.product_kind = 'policy'
          AND product.logical_id = 'policy:versioned-test'
    ) <> 2 THEN
        RAISE EXCEPTION 'Policy Product versions were collapsed';
    END IF;

    digest := pg_catalog.md5('versioned:first:policy');
    first_object_id := 'blake3:' || digest || digest;
    digest := pg_catalog.md5('versioned:second:policy');
    second_object_id := 'blake3:' || digest || digest;
    INSERT INTO policy_set_entries (
        space_id, policy_set_id, ordinal, policy_id, policy_version_id,
        policy_product_object_id, dependency_object_ids_jcs
    ) VALUES (
        'space:a',
        'blake3:2222222222222222222222222222222222222222222222222222222222222222',
        0, 'policy:versioned-test', 'policy-version:first', first_object_id, '[]'
    );
    rejected := false;
    BEGIN
        INSERT INTO policy_set_entries (
            space_id, policy_set_id, ordinal, policy_id, policy_version_id,
            policy_product_object_id, dependency_object_ids_jcs
        ) VALUES (
            'space:a',
            'blake3:2222222222222222222222222222222222222222222222222222222222222222',
            1, 'policy:versioned-test', 'policy-version:second', second_object_id, '[]'
        );
    EXCEPTION WHEN unique_violation THEN rejected := true;
    END;
    IF NOT rejected THEN
        RAISE EXCEPTION 'PolicySet accepted two active versions of one logical Policy';
    END IF;
END
$product_identity_boundaries$;

DO $layer_definition_rejections$
DECLARE rejected BOOLEAN;
BEGIN
    rejected := false;
    BEGIN
        PERFORM layrs_cas_layer_definition(
            'space:a', 'layer:l1', NULL, 0,
            'blake3:9999999999999999999999999999999999999999999999999999999999999999'
        );
    EXCEPTION WHEN foreign_key_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'missing Layer Product was accepted'; END IF;

    rejected := false;
    BEGIN
        PERFORM layrs_cas_layer_definition(
            'space:a', 'layer:l1', NULL, 0,
            'blake3:5555555555555555555555555555555555555555555555555555555555555555'
        );
    EXCEPTION WHEN foreign_key_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'wrong-kind Layer Product was accepted'; END IF;

    rejected := false;
    BEGIN
        PERFORM layrs_cas_layer_definition(
            'space:a', 'layer:l1', NULL, 0,
            'blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
        );
    EXCEPTION WHEN foreign_key_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'cross-Space Layer Product was accepted'; END IF;
END
$layer_definition_rejections$;

SELECT writer_role_name AS writer_role FROM schema_metadata WHERE singleton_id = 1 \gset
SET ROLE :"writer_role";
SELECT layrs_cas_layer_definition(
    'space:a', 'layer:l1', NULL, 0,
    'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
);
SELECT layrs_cas_layer_definition(
    'space:a', 'layer:l1',
    'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 0,
    'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'
);

DO $writer_dml_boundary$
DECLARE rejected BOOLEAN;
BEGIN
    rejected := false;
    BEGIN
        INSERT INTO current_layer_definitions (
            space_id, layer_id, current_product_object_id, generation
        ) VALUES (
            'space:a', 'layer:forged',
            'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 99
        );
    EXCEPTION WHEN insufficient_privilege THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'writer directly inserted Layer authority'; END IF;

    rejected := false;
    BEGIN
        UPDATE current_layer_definitions SET generation = 99
        WHERE space_id = 'space:a' AND layer_id = 'layer:l1';
    EXCEPTION WHEN insufficient_privilege THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'writer directly updated Layer authority'; END IF;

    rejected := false;
    BEGIN
        DELETE FROM current_layer_definitions
        WHERE space_id = 'space:a' AND layer_id = 'layer:l1';
    EXCEPTION WHEN insufficient_privilege THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'writer directly deleted Layer authority'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO layer_definition_history (
            space_id, layer_id, generation, product_object_id
        ) VALUES (
            'space:a', 'layer:l1', 99,
            'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'
        );
    EXCEPTION WHEN insufficient_privilege THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'writer directly inserted Layer history'; END IF;

    rejected := false;
    BEGIN
        UPDATE layer_definition_history SET generation = 99
        WHERE space_id = 'space:a' AND layer_id = 'layer:l1' AND generation = 1;
    EXCEPTION WHEN insufficient_privilege THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'writer directly updated Layer history'; END IF;

    rejected := false;
    BEGIN
        DELETE FROM layer_definition_history
        WHERE space_id = 'space:a' AND layer_id = 'layer:l1' AND generation = 1;
    EXCEPTION WHEN insufficient_privilege THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'writer directly deleted Layer history'; END IF;
END
$writer_dml_boundary$;

CREATE TEMP TABLE products (
    space_id TEXT,
    product_kind TEXT,
    logical_id TEXT,
    product_object_id TEXT,
    previous_product_object_id TEXT
);
CREATE TEMP TABLE current_layer_definitions (
    space_id TEXT,
    layer_id TEXT,
    current_product_object_id TEXT,
    generation BIGINT,
    updated_transaction_id TEXT
);
CREATE TEMP TABLE layer_definition_history (
    space_id TEXT,
    layer_id TEXT,
    generation BIGINT,
    product_object_id TEXT,
    previous_product_object_id TEXT,
    transaction_id TEXT
);
INSERT INTO pg_temp.products VALUES (
    'space:a', 'layer', 'layer:l1',
    'blake3:9999999999999999999999999999999999999999999999999999999999999999',
    'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'
);
INSERT INTO pg_temp.current_layer_definitions VALUES (
    'space:a', 'layer:l1',
    'blake3:9999999999999999999999999999999999999999999999999999999999999999',
    999, NULL
);

DO $pg_temp_hijack_rejected$
DECLARE rejected BOOLEAN := false;
BEGIN
    BEGIN
        PERFORM layrs_cas_layer_definition(
            'space:a', 'layer:l1',
            'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc', 1,
            'blake3:9999999999999999999999999999999999999999999999999999999999999999'
        );
    EXCEPTION WHEN foreign_key_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'pg_temp Product hijacked SECURITY DEFINER CAS'; END IF;
    IF (SELECT generation FROM pg_temp.current_layer_definitions) <> 999 THEN
        RAISE EXCEPTION 'pg_temp current authority was mutated';
    END IF;
    IF (SELECT count(*) FROM pg_temp.layer_definition_history) <> 0 THEN
        RAISE EXCEPTION 'pg_temp history was mutated';
    END IF;
END
$pg_temp_hijack_rejected$;

SELECT layrs_cas_layer_definition(
    'space:a', 'layer:l1',
    'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc', 1,
    'blake3:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd'
);
DO $pg_temp_hijack_real_write$
BEGIN
    IF (
        SELECT generation FROM public.current_layer_definitions
        WHERE space_id = 'space:a' AND layer_id = 'layer:l1'
    ) <> 2 THEN
        RAISE EXCEPTION 'valid CAS did not update real Layer authority';
    END IF;
    IF (SELECT generation FROM pg_temp.current_layer_definitions) <> 999 THEN
        RAISE EXCEPTION 'valid CAS wrote pg_temp authority';
    END IF;
    IF (
        SELECT count(*) FROM public.layer_definition_history
        WHERE space_id = 'space:a' AND layer_id = 'layer:l1'
    ) <> 3 THEN
        RAISE EXCEPTION 'valid CAS did not append real Layer history';
    END IF;
END
$pg_temp_hijack_real_write$;
RESET ROLE;

DO $layer_definition_cas$
DECLARE
    history_rows BIGINT;
    rejected BOOLEAN := false;
BEGIN
    SELECT count(*) INTO history_rows
    FROM public.layer_definition_history
    WHERE space_id = 'space:a' AND layer_id = 'layer:l1';
    IF history_rows <> 3 THEN RAISE EXCEPTION 'Layer definition history is incomplete'; END IF;

    BEGIN
        PERFORM layrs_cas_layer_definition(
            'space:a', 'layer:l1',
            'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 0,
            'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc'
        );
    EXCEPTION WHEN serialization_failure THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'stale Layer definition CAS was accepted'; END IF;

    rejected := false;
    BEGIN
        UPDATE public.layer_definition_history SET generation = 2
        WHERE space_id = 'space:a' AND layer_id = 'layer:l1' AND generation = 1;
    EXCEPTION WHEN SQLSTATE '55000' THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'Layer definition history mutation was accepted'; END IF;
END
$layer_definition_cas$;

INSERT INTO object_plans (
    plan_id, request_id, space_id, client_id, principal_id,
    authorization_epoch, request_descriptors_digest, expires_at
) VALUES (
    'plan:object-valid', 'request:object-valid', 'space:a', 'client:test', 'principal:alice',
    1, 'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', now() + interval '1 hour'
);
INSERT INTO object_plan_items (
    plan_id, ordinal, object_id, object_kind, canonical_byte_length,
    encoded_byte_length, compression, descriptor_digest, disposition
) VALUES
    (
        'plan:object-valid', 0,
        'blake3:8888888888888888888888888888888888888888888888888888888888888888',
        'tree', 2, 2, 'identity',
        'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab', 'present'
    ),
    (
        'plan:object-valid', 1,
        'blake3:9999999999999999999999999999999999999999999999999999999999999999',
        'tree', 2, 2, 'identity',
        'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaac', 'missing'
    );

DO $upload_rejections$
DECLARE rejected BOOLEAN;
BEGIN
    rejected := false;
    BEGIN
        INSERT INTO upload_capabilities (
            capability_id, plan_id, item_ordinal, object_id, descriptor_digest,
            space_id, client_id, request_id, principal_id, authorization_epoch,
            token_digest, method, expires_at
        ) VALUES (
            'capability:present', 'plan:object-valid', 0,
            'blake3:8888888888888888888888888888888888888888888888888888888888888888',
            'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab',
            'space:a', 'client:test', 'request:object-valid', 'principal:alice', 1,
            'token-present', 'PUT', now() + interval '1 hour'
        );
    EXCEPTION WHEN foreign_key_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'present object received upload capability'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO upload_capabilities (
            capability_id, plan_id, item_ordinal, object_id, descriptor_digest,
            space_id, client_id, request_id, principal_id, authorization_epoch,
            token_digest, method, expires_at
        ) VALUES (
            'capability:wrong-actor', 'plan:object-valid', 1,
            'blake3:9999999999999999999999999999999999999999999999999999999999999999',
            'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaac',
            'space:a', 'client:test', 'request:object-valid', 'principal:bob', 1,
            'token-wrong-actor', 'PUT', now() + interval '1 hour'
        );
    EXCEPTION WHEN foreign_key_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'mismatched upload capability binding accepted'; END IF;
END
$upload_rejections$;

INSERT INTO upload_capabilities (
    capability_id, plan_id, item_ordinal, object_id, descriptor_digest,
    space_id, client_id, request_id, principal_id, authorization_epoch,
    token_digest, method, expires_at
) VALUES (
    'capability:upload-valid', 'plan:object-valid', 1,
    'blake3:9999999999999999999999999999999999999999999999999999999999999999',
    'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaac',
    'space:a', 'client:test', 'request:object-valid', 'principal:alice', 1,
    'token-upload-valid', 'PUT', now() + interval '1 hour'
);
UPDATE object_plans
SET plan_state = 'finalized', finalized_at = now()
WHERE plan_id = 'plan:object-valid';

INSERT INTO object_plans (
    plan_id, request_id, space_id, client_id, principal_id,
    authorization_epoch, request_descriptors_digest, expires_at
) VALUES (
    'plan:object-incomplete', 'request:object-incomplete', 'space:a', 'client:test', 'principal:alice',
    1, 'blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', now() + interval '1 hour'
);
INSERT INTO object_plan_items (
    plan_id, ordinal, object_id, object_kind, canonical_byte_length,
    encoded_byte_length, compression, descriptor_digest, disposition
) VALUES (
    'plan:object-incomplete', 0,
    'blake3:9999999999999999999999999999999999999999999999999999999999999999',
    'tree', 2, 2, 'identity',
    'blake3:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbc', 'missing'
);
DO $object_bijection$
DECLARE rejected BOOLEAN := false;
BEGIN
    BEGIN
        UPDATE object_plans SET plan_state = 'finalized', finalized_at = now()
        WHERE plan_id = 'plan:object-incomplete';
        SET CONSTRAINTS ALL IMMEDIATE;
    EXCEPTION WHEN check_violation THEN rejected := true;
    END;
    SET CONSTRAINTS ALL DEFERRED;
    IF NOT rejected THEN RAISE EXCEPTION 'incomplete object plan finalized'; END IF;
END
$object_bijection$;

INSERT INTO fetch_plans (
    plan_id, request_id, space_id, client_id, principal_id,
    authorization_epoch, request_claims_digest, expires_at
) VALUES (
    'plan:fetch-valid', 'request:fetch-valid', 'space:a', 'client:test', 'principal:alice',
    1, 'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc', now() + interval '1 hour'
);
INSERT INTO fetch_plan_claims (
    plan_id, ordinal, claim_kind, claim_canonical_jcs, decision, denied_code
) VALUES
    ('plan:fetch-valid', 0, 'object', '{}', 'denied', 'not_reachable'),
    ('plan:fetch-valid', 1, 'object', '{}', 'allowed', NULL),
    ('plan:fetch-valid', 2, 'object', '{}', 'denied', 'path_denied'),
    ('plan:fetch-valid', 3, 'object', '{}', 'denied', 'audit_denied'),
    ('plan:fetch-valid', 4, 'object', '{}', 'denied', 'not_available'),
    ('plan:fetch-valid', 5, 'object', '{}', 'denied', 'invalid_root'),
    ('plan:fetch-valid', 6, 'object', '{}', 'denied', 'invalid_path'),
    ('plan:fetch-valid', 7, 'object', '{}', 'denied', 'no_verified_manifest');
INSERT INTO fetch_claim_objects (
    plan_id, claim_ordinal, object_ordinal, object_id, descriptor_digest
) VALUES (
    'plan:fetch-valid', 1, 0,
    'blake3:8888888888888888888888888888888888888888888888888888888888888888',
    'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccd'
);

DO $fetch_rejections$
DECLARE rejected BOOLEAN;
BEGIN
    rejected := false;
    BEGIN
        INSERT INTO fetch_plan_claims (
            plan_id, ordinal, claim_kind, claim_canonical_jcs, decision, denied_code
        ) VALUES (
            'plan:fetch-valid', 90, 'object', '{}', 'allowed', 'path_denied'
        );
    EXCEPTION WHEN check_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'allowed claim accepted a denied code'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO fetch_plan_claims (
            plan_id, ordinal, claim_kind, claim_canonical_jcs, decision, denied_code
        ) VALUES (
            'plan:fetch-valid', 91, 'object', '{}', 'denied', NULL
        );
    EXCEPTION WHEN check_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'denied claim accepted a null denied code'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO fetch_plan_claims (
            plan_id, ordinal, claim_kind, claim_canonical_jcs, decision, denied_code
        ) VALUES (
            'plan:fetch-valid', 92, 'object', '{}', 'denied', 'policy_denied'
        );
    EXCEPTION WHEN check_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'claim accepted an unknown denied code'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO fetch_capabilities (
            capability_id, plan_id, claim_ordinal, object_id, descriptor_digest,
            space_id, client_id, request_id, principal_id, authorization_epoch,
            token_digest, resume_token_digest, method, expires_at
        ) VALUES (
            'capability:denied', 'plan:fetch-valid', 0,
            'blake3:8888888888888888888888888888888888888888888888888888888888888888',
            'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccd',
            'space:a', 'client:test', 'request:fetch-valid', 'principal:alice', 1,
            'token-fetch-denied', 'resume-fetch-denied', 'GET', now() + interval '1 hour'
        );
    EXCEPTION WHEN foreign_key_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'denied claim received fetch capability'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO fetch_capabilities (
            capability_id, plan_id, claim_ordinal, object_id, descriptor_digest,
            space_id, client_id, request_id, principal_id, authorization_epoch,
            token_digest, resume_token_digest, method, expires_at
        ) VALUES (
            'capability:fetch-wrong', 'plan:fetch-valid', 1,
            'blake3:8888888888888888888888888888888888888888888888888888888888888888',
            'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccd',
            'space:b', 'client:test', 'request:fetch-valid', 'principal:alice', 1,
            'token-fetch-wrong', 'resume-fetch-wrong', 'GET', now() + interval '1 hour'
        );
    EXCEPTION WHEN foreign_key_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'mismatched fetch capability binding accepted'; END IF;
END
$fetch_rejections$;

INSERT INTO fetch_capabilities (
    capability_id, plan_id, claim_ordinal, object_id, descriptor_digest,
    space_id, client_id, request_id, principal_id, authorization_epoch,
    token_digest, resume_token_digest, method, expires_at
) VALUES (
    'capability:fetch-valid', 'plan:fetch-valid', 1,
    'blake3:8888888888888888888888888888888888888888888888888888888888888888',
    'blake3:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccd',
    'space:a', 'client:test', 'request:fetch-valid', 'principal:alice', 1,
    'token-fetch-valid', 'resume-fetch-valid', 'GET', now() + interval '1 hour'
);
UPDATE fetch_plans
SET plan_state = 'finalized', finalized_at = now()
WHERE plan_id = 'plan:fetch-valid';
DO $fetch_denied_code_exact_replay$
DECLARE replayed TEXT[];
BEGIN
    SELECT pg_catalog.array_agg(denied_code ORDER BY ordinal)
    INTO replayed
    FROM fetch_plan_claims
    WHERE plan_id = 'plan:fetch-valid' AND decision = 'denied';
    IF replayed <> ARRAY[
        'not_reachable', 'path_denied', 'audit_denied', 'not_available',
        'invalid_root', 'invalid_path', 'no_verified_manifest'
    ]::TEXT[] THEN
        RAISE EXCEPTION 'denied fetch decisions did not replay exactly: %', replayed;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM fetch_plan_claims
        WHERE plan_id = 'plan:fetch-valid' AND ordinal = 1
          AND decision = 'allowed' AND denied_code IS NULL
    ) THEN
        RAISE EXCEPTION 'allowed fetch decision did not replay exactly';
    END IF;
END
$fetch_denied_code_exact_replay$;

INSERT INTO fetch_plans (
    plan_id, request_id, space_id, client_id, principal_id,
    authorization_epoch, request_claims_digest, expires_at
) VALUES (
    'plan:fetch-incomplete', 'request:fetch-incomplete', 'space:a', 'client:test', 'principal:alice',
    1, 'blake3:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd', now() + interval '1 hour'
);
INSERT INTO fetch_plan_claims (
    plan_id, ordinal, claim_kind, claim_canonical_jcs, decision
) VALUES (
    'plan:fetch-incomplete', 0, 'object', '{}', 'allowed'
);
INSERT INTO fetch_claim_objects (
    plan_id, claim_ordinal, object_ordinal, object_id, descriptor_digest
) VALUES (
    'plan:fetch-incomplete', 0, 0,
    'blake3:8888888888888888888888888888888888888888888888888888888888888888',
    'blake3:ddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddde'
);
DO $fetch_bijection$
DECLARE rejected BOOLEAN := false;
BEGIN
    BEGIN
        UPDATE fetch_plans SET plan_state = 'finalized', finalized_at = now()
        WHERE plan_id = 'plan:fetch-incomplete';
        SET CONSTRAINTS ALL IMMEDIATE;
    EXCEPTION WHEN check_violation THEN rejected := true;
    END;
    SET CONSTRAINTS ALL DEFERRED;
    IF NOT rejected THEN RAISE EXCEPTION 'incomplete fetch plan finalized'; END IF;
END
$fetch_bijection$;

INSERT INTO fetch_plans (
    plan_id, request_id, space_id, client_id, principal_id,
    authorization_epoch, request_claims_digest, expires_at
) VALUES (
    'plan:fetch-file', 'request:fetch-file', 'space:a', 'client:test', 'principal:alice',
    1, 'blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeea', now() + interval '1 hour'
);
INSERT INTO fetch_plan_claims (
    plan_id, ordinal, claim_kind, claim_canonical_jcs, decision,
    denied_code, selected_file_manifest_id
) VALUES
    (
        'plan:fetch-file', 0, 'file', '{}', 'allowed', NULL,
        'blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee'
    ),
    ('plan:fetch-file', 1, 'object', '{}', 'allowed', NULL, NULL),
    ('plan:fetch-file', 2, 'object', '{}', 'denied', 'path_denied', NULL);
INSERT INTO fetch_claim_objects (
    plan_id, claim_ordinal, object_ordinal, object_id, descriptor_digest
) VALUES
    (
        'plan:fetch-file', 0, 0,
        'blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
        'blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeb'
    ),
    (
        'plan:fetch-file', 0, 1,
        'blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff',
        'blake3:fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa'
    ),
    (
        'plan:fetch-file', 0, 2,
        'blake3:0000000000000000000000000000000000000000000000000000000000000000',
        'blake3:000000000000000000000000000000000000000000000000000000000000000a'
    ),
    (
        'plan:fetch-file', 1, 0,
        'blake3:8888888888888888888888888888888888888888888888888888888888888888',
        'blake3:888888888888888888888888888888888888888888888888888888888888888a'
    );

DO $fetch_object_set_rejections$
DECLARE rejected BOOLEAN;
BEGIN
    rejected := false;
    BEGIN
        INSERT INTO fetch_claim_objects (
            plan_id, claim_ordinal, object_ordinal, object_id, descriptor_digest
        ) VALUES (
            'plan:fetch-file', 2, 0,
            'blake3:8888888888888888888888888888888888888888888888888888888888888888',
            'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaad'
        );
    EXCEPTION WHEN foreign_key_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'denied claim accepted an expected object'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO fetch_capabilities (
            capability_id, plan_id, claim_ordinal, object_id, descriptor_digest,
            space_id, client_id, request_id, principal_id, authorization_epoch,
            token_digest, resume_token_digest, method, expires_at
        ) VALUES (
            'capability:cross-claim', 'plan:fetch-file', 0,
            'blake3:8888888888888888888888888888888888888888888888888888888888888888',
            'blake3:888888888888888888888888888888888888888888888888888888888888888a',
            'space:a', 'client:test', 'request:fetch-file', 'principal:alice', 1,
            'token-cross-claim', 'resume-cross-claim', 'GET', now() + interval '1 hour'
        );
    EXCEPTION WHEN foreign_key_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'cross-claim capability was accepted'; END IF;

    rejected := false;
    BEGIN
        INSERT INTO fetch_capabilities (
            capability_id, plan_id, claim_ordinal, object_id, descriptor_digest,
            space_id, client_id, request_id, principal_id, authorization_epoch,
            token_digest, resume_token_digest, method, expires_at
        ) VALUES (
            'capability:extra-object', 'plan:fetch-file', 0,
            'blake3:8888888888888888888888888888888888888888888888888888888888888888',
            'blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaae',
            'space:a', 'client:test', 'request:fetch-file', 'principal:alice', 1,
            'token-extra-object', 'resume-extra-object', 'GET', now() + interval '1 hour'
        );
    EXCEPTION WHEN foreign_key_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'unexpected fetch object capability was accepted'; END IF;
END
$fetch_object_set_rejections$;

INSERT INTO fetch_capabilities (
    capability_id, plan_id, claim_ordinal, object_id, descriptor_digest,
    space_id, client_id, request_id, principal_id, authorization_epoch,
    token_digest, resume_token_digest, method, expires_at
) VALUES
    (
        'capability:file-manifest', 'plan:fetch-file', 0,
        'blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
        'blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeb',
        'space:a', 'client:test', 'request:fetch-file', 'principal:alice', 1,
        'token-file-manifest', 'resume-file-manifest', 'GET', now() + interval '1 hour'
    ),
    (
        'capability:file-chunk-one', 'plan:fetch-file', 0,
        'blake3:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff',
        'blake3:fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa',
        'space:a', 'client:test', 'request:fetch-file', 'principal:alice', 1,
        'token-file-chunk-one', 'resume-file-chunk-one', 'GET', now() + interval '1 hour'
    ),
    (
        'capability:file-chunk-two', 'plan:fetch-file', 0,
        'blake3:0000000000000000000000000000000000000000000000000000000000000000',
        'blake3:000000000000000000000000000000000000000000000000000000000000000a',
        'space:a', 'client:test', 'request:fetch-file', 'principal:alice', 1,
        'token-file-chunk-two', 'resume-file-chunk-two', 'GET', now() + interval '1 hour'
    ),
    (
        'capability:file-second-claim', 'plan:fetch-file', 1,
        'blake3:8888888888888888888888888888888888888888888888888888888888888888',
        'blake3:888888888888888888888888888888888888888888888888888888888888888a',
        'space:a', 'client:test', 'request:fetch-file', 'principal:alice', 1,
        'token-file-second-claim', 'resume-file-second-claim', 'GET', now() + interval '1 hour'
    );

DO $duplicate_fetch_capability$
DECLARE rejected BOOLEAN := false;
BEGIN
    BEGIN
        INSERT INTO fetch_capabilities (
            capability_id, plan_id, claim_ordinal, object_id, descriptor_digest,
            space_id, client_id, request_id, principal_id, authorization_epoch,
            token_digest, resume_token_digest, method, expires_at
        ) VALUES (
            'capability:file-manifest-duplicate', 'plan:fetch-file', 0,
            'blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee',
            'blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeb',
            'space:a', 'client:test', 'request:fetch-file', 'principal:alice', 1,
            'token-file-manifest-duplicate', 'resume-file-manifest-duplicate', 'GET', now() + interval '1 hour'
        );
    EXCEPTION WHEN unique_violation THEN rejected := true;
    END;
    IF NOT rejected THEN RAISE EXCEPTION 'duplicate per-object fetch capability was accepted'; END IF;
END
$duplicate_fetch_capability$;

UPDATE fetch_plans
SET plan_state = 'finalized', finalized_at = now()
WHERE plan_id = 'plan:fetch-file';

INSERT INTO fetch_plans (
    plan_id, request_id, space_id, client_id, principal_id,
    authorization_epoch, request_claims_digest, expires_at
) VALUES (
    'plan:fetch-empty-allowed', 'request:fetch-empty-allowed', 'space:a', 'client:test', 'principal:alice',
    1, 'blake3:abababababababababababababababababababababababababababababababab', now() + interval '1 hour'
);
INSERT INTO fetch_plan_claims (plan_id, ordinal, claim_kind, claim_canonical_jcs, decision)
VALUES ('plan:fetch-empty-allowed', 0, 'object', '{}', 'allowed');
DO $fetch_empty_allowed$
DECLARE rejected BOOLEAN := false;
BEGIN
    BEGIN
        UPDATE fetch_plans SET plan_state = 'finalized', finalized_at = now()
        WHERE plan_id = 'plan:fetch-empty-allowed';
        SET CONSTRAINTS ALL IMMEDIATE;
    EXCEPTION WHEN check_violation THEN rejected := true;
    END;
    SET CONSTRAINTS ALL DEFERRED;
    IF NOT rejected THEN RAISE EXCEPTION 'allowed claim without expected objects finalized'; END IF;
END
$fetch_empty_allowed$;

DO $fingerprint_stable_after_data$
DECLARE stored TEXT; actual TEXT;
BEGIN
    SELECT catalog_fingerprint, layrs_schema_catalog_fingerprint()
    INTO stored, actual
    FROM schema_metadata WHERE singleton_id = 1;
    IF stored <> actual THEN
        RAISE EXCEPTION 'catalog fingerprint changed after data-only fixture: % <> %', stored, actual;
    END IF;
END
$fingerprint_stable_after_data$;

SELECT 'schema_v3_p1_fixture_ok';
