import { randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";

const image = process.env.LAYRS_TEST_POSTGRES_IMAGE ?? "postgres:16-alpine";
const container = `layrs-schema-v3-${process.pid}-${randomUUID().slice(0, 8)}`;
const baseline = resolve("crates/layrs-store-server/migrations/0001_sync_v3_baseline.sql");
const fixture = resolve("crates/layrs-store-server/tests/fixtures/schema_v3_p1.sql");
let started = false;

try {
  runDocker([
    "run", "--rm", "--detach", "--name", container,
    "--env", "POSTGRES_PASSWORD=layrs-schema-test", image
  ]);
  started = true;
  waitForPostgres();
  runDocker(["cp", baseline, `${container}:/tmp/baseline.sql`]);
  runDocker(["cp", fixture, `${container}:/tmp/schema_v3_p1.sql`]);

  createDatabase("valid");
  applyBaseline("valid");
  psql("valid", ["-f", "/tmp/schema_v3_p1.sql"]);
  assertEqual(
    query(
      "valid",
      "SELECT p.actor_id || '|' || e.actor_kind || '|' || e.actor_id || '|' || e.device_id FROM principals p CROSS JOIN space_events e WHERE p.principal_id='principal:alice' AND e.event_id='event:actor-exact'"
    ),
    "actor:alice|account|actor:alice|device:desktop-a",
    "durable PrincipalId-to-ActorId mapping and exact EventActor replay"
  );
  assertEqual(
    query(
      "valid",
      "SELECT string_agg(denied_code, ',' ORDER BY ordinal) FROM fetch_plan_claims WHERE plan_id='plan:fetch-valid' AND decision='denied'"
    ),
    "not_reachable,path_denied,audit_denied,not_available,invalid_root,invalid_path,no_verified_manifest",
    "exact denied fetch decision replay"
  );
  assertEqual(
    query(
      "valid",
      "SELECT count(*) FROM (SELECT space_id,product_kind,logical_id FROM products WHERE product_kind IN ('marker','release','proof','gateEvaluation','weave','decision') GROUP BY space_id,product_kind,logical_id HAVING count(*) > 1) duplicates"
    ),
    "0",
    "singleton Product logical identities"
  );
  assertEqual(
    query(
      "valid",
      "SELECT (SELECT count(*) FROM products WHERE space_id='space:a' AND product_kind='layer' AND logical_id='layer:l1') || '|' || (SELECT count(*) FROM products WHERE space_id='space:a' AND product_kind='gateDefinition' AND logical_id='gate-definition:versioned-test') || '|' || (SELECT count(*) FROM products WHERE space_id='space:a' AND product_kind='policy' AND logical_id='policy:versioned-test')"
    ),
    "3|2|2",
    "versioned Layer, GateDefinition, and Policy Products"
  );

  for (const [database, setup, expectedOnlyTable] of [
    ["arbitrary", "CREATE TABLE surprise (value TEXT);", "surprise"],
    ["audit", "CREATE TABLE audit_events (event_id TEXT);", "audit_events"],
    ["partial", "CREATE TABLE schema_metadata (legacy_value TEXT);", "schema_metadata"]
  ]) {
    createDatabase(database);
    psql(database, ["-c", setup]);
    const failure = applyBaseline(database, false);
    assertIncludes(failure.stderr, "target schema is not empty", `${database} fresh-only error`);
    const tables = query(
      database,
      "SELECT string_agg(tablename, ',' ORDER BY tablename) FROM pg_tables WHERE schemaname='public'"
    );
    assertEqual(tables, expectedOnlyTable, `${database} must remain unmodified`);
  }

  for (const [database, setup, survivorQuery] of [
    [
      "function_only",
      "CREATE FUNCTION surprise() RETURNS integer LANGUAGE sql AS 'SELECT 1';",
      "SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public' AND p.proname='surprise'"
    ],
    [
      "enum_only",
      "CREATE TYPE surprise_enum AS ENUM ('one', 'two');",
      "SELECT count(*) FROM pg_type t JOIN pg_namespace n ON n.oid=t.typnamespace WHERE n.nspname='public' AND t.typname='surprise_enum'"
    ],
    [
      "domain_only",
      "CREATE DOMAIN surprise_domain AS text CHECK (VALUE <> '');",
      "SELECT count(*) FROM pg_type t JOIN pg_namespace n ON n.oid=t.typnamespace WHERE n.nspname='public' AND t.typname='surprise_domain'"
    ]
  ]) {
    createDatabase(database);
    psql(database, ["-c", setup]);
    const failure = applyBaseline(database, false);
    assertIncludes(failure.stderr, "target schema is not empty", `${database} fresh-only error`);
    assertEqual(query(database, survivorQuery), "1", `${database} object survives refusal`);
    assertEqual(
      query(database, "SELECT count(*) FROM pg_tables WHERE schemaname='public'"),
      "0",
      `${database} refusal creates no V3 table`
    );
  }

  createDatabase("dangerous_role");
  const dangerousRoleName = query(
    "dangerous_role",
    "SELECT 'layrs_v3_w_' || substr(md5(current_database() || '/' || current_schema()), 1, 24)"
  );
  psql("dangerous_role", [
    "-c",
    `CREATE ROLE ${dangerousRoleName} LOGIN SUPERUSER CREATEDB CREATEROLE INHERIT REPLICATION BYPASSRLS`
  ]);
  const dangerousRoleFailure = applyBaseline("dangerous_role", false);
  assertIncludes(
    dangerousRoleFailure.stderr,
    "unsafe pre-existing Layrs Sync V3 writer role",
    "privileged pre-existing writer role"
  );
  assertEqual(
    query("dangerous_role", "SELECT count(*) FROM pg_tables WHERE schemaname='public'"),
    "0",
    "writer role refusal occurs before schema mutation"
  );

  createDatabase("multi_schema_role_names");
  psql("multi_schema_role_names", ["-c", "CREATE SCHEMA second_installation"]);
  const roleNames = query(
    "multi_schema_role_names",
    "SELECT ('layrs_v3_w_' || substr(md5(current_database() || '/public'), 1, 24)) <> ('layrs_v3_w_' || substr(md5(current_database() || '/second_installation'), 1, 24))"
  );
  assertEqual(roleNames, "t", "writer role names are installation-scoped");

  for (const [database, mutation] of [
    ["drift_table", "DROP TABLE space_events;"],
    [
      "drift_constraint",
      "ALTER TABLE object_leases DROP CONSTRAINT object_leases_fencing_generation_check;"
    ],
    ["drift_index", "DROP INDEX space_events_by_transaction;"],
    ["drift_view_add", "CREATE VIEW surprise_view AS SELECT 1 AS value;"],
    ["drift_sequence_add", "CREATE SEQUENCE surprise_sequence;"],
    ["drift_enum_add", "CREATE TYPE surprise_drift_enum AS ENUM ('one', 'two');"]
  ]) {
    createDatabase(database);
    applyBaseline(database);
    const before = query(
      database,
      "SELECT catalog_fingerprint || '|' || layrs_schema_catalog_fingerprint() FROM schema_metadata"
    );
    const [stored, live] = before.split("|");
    assertEqual(stored, live, `${database} starts without drift`);
    psql(database, ["-c", mutation]);
    const after = query(
      database,
      "SELECT catalog_fingerprint || '|' || layrs_schema_catalog_fingerprint() FROM schema_metadata"
    );
    const [storedAfter, liveAfter] = after.split("|");
    if (storedAfter === liveAfter) {
      throw new Error(`${database} mutation was not detected by catalog fingerprint`);
    }
  }

  for (const [database, mutation] of [
    [
      "drift_function_add",
      "CREATE FUNCTION surprise() RETURNS integer LANGUAGE sql AS 'SELECT 1';"
    ],
    [
      "drift_function_replace",
      "CREATE OR REPLACE FUNCTION layrs_validate_fetch_plan_bijection() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN IF TG_OP = ''DELETE'' THEN RETURN OLD; END IF; RETURN NEW; END'"
    ]
  ]) {
    createDatabase(database);
    applyBaseline(database);
    psql(database, ["-c", mutation]);
    const [stored, live] = query(
      database,
      "SELECT catalog_fingerprint || '|' || layrs_schema_catalog_fingerprint() FROM schema_metadata"
    ).split("|");
    if (stored === live) {
      throw new Error(`${database} routine mutation was not detected`);
    }
  }

  createDatabase("drift_function_drop");
  applyBaseline("drift_function_drop");
  psql("drift_function_drop", ["-c", "DROP FUNCTION layrs_schema_catalog_fingerprint();"]);
  const missingProbe = psql(
    "drift_function_drop",
    ["-c", "SELECT layrs_schema_catalog_fingerprint();"],
    false
  );
  assertIncludes(missingProbe.stderr, "does not exist", "dropped fingerprint function fails closed");

  createDatabase("drift_view_replace");
  applyBaseline("drift_view_replace");
  psql("drift_view_replace", ["-c", "CREATE VIEW surprise_view AS SELECT 1 AS value;"]);
  const addedViewFingerprint = query(
    "drift_view_replace",
    "SELECT layrs_schema_catalog_fingerprint()"
  );
  psql("drift_view_replace", [
    "-c", "CREATE OR REPLACE VIEW surprise_view AS SELECT 2 AS value;"
  ]);
  const replacedViewFingerprint = query(
    "drift_view_replace",
    "SELECT layrs_schema_catalog_fingerprint()"
  );
  if (addedViewFingerprint === replacedViewFingerprint) {
    throw new Error("view replacement was not detected by catalog fingerprint");
  }

  const catalogFingerprint = query(
    "valid",
    "SELECT catalog_fingerprint FROM schema_metadata WHERE singleton_id=1"
  );
  console.log(
    `Layrs Sync V3 PostgreSQL schema: fresh-only, fingerprint ${catalogFingerprint}, ` +
      "typed roots and capabilities OK."
  );
} finally {
  if (started) {
    spawnSync("docker", ["rm", "--force", container], {
      encoding: "utf8",
      windowsHide: true,
      stdio: "ignore"
    });
  }
}

function waitForPostgres() {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    const result = docker([
      "exec", container, "pg_isready", "-U", "postgres", "-d", "postgres"
    ]);
    if (result.status === 0) return;
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 250);
  }
  throw new Error("PostgreSQL 16 test container did not become ready");
}

function createDatabase(database) {
  runDocker(["exec", container, "createdb", "-U", "postgres", database]);
}

function applyBaseline(database, mustSucceed = true) {
  return psql(database, ["-f", "/tmp/baseline.sql"], mustSucceed);
}

function query(database, sql) {
  return psql(database, ["-Atc", sql]).stdout.trim();
}

function psql(database, args, mustSucceed = true) {
  const result = docker([
    "exec", container, "psql", "-v", "ON_ERROR_STOP=1", "-U", "postgres", "-d", database,
    ...args
  ]);
  if (mustSucceed && result.status !== 0) failCommand(result);
  if (!mustSucceed && result.status === 0) {
    throw new Error(`expected PostgreSQL command to fail: ${args.join(" ")}`);
  }
  return result;
}

function runDocker(args) {
  const result = docker(args);
  if (result.status !== 0) failCommand(result);
  return result;
}

function docker(args) {
  return spawnSync("docker", args, {
    cwd: process.cwd(),
    encoding: "utf8",
    windowsHide: true
  });
}

function failCommand(result) {
  throw new Error([result.stdout, result.stderr].filter(Boolean).join("\n").trim());
}

function assertEqual(actual, expected, context) {
  if (actual !== expected) {
    throw new Error(`${context}: expected ${JSON.stringify(expected)}, received ${JSON.stringify(actual)}`);
  }
}

function assertIncludes(actual, expected, context) {
  if (!actual.includes(expected)) {
    throw new Error(`${context}: expected ${JSON.stringify(expected)} in ${JSON.stringify(actual)}`);
  }
}
