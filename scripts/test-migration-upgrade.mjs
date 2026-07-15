import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, resolve } from "node:path";

const migrationPath = "crates/layrs-store-server/migrations";
const databaseUrl = process.env.LAYRS_TEST_DATABASE_URL;
const baseRef = process.env.LAYRS_MIGRATION_BASE_REF ?? process.argv[2];

if (!databaseUrl) {
  fail("LAYRS_TEST_DATABASE_URL is required and must point to a disposable empty database");
}
if (process.env.LAYRS_TEST_DATABASE_ISOLATED !== "1") {
  fail("set LAYRS_TEST_DATABASE_ISOLATED=1 to acknowledge that the upgrade database is disposable");
}
if (!baseRef) {
  fail("set LAYRS_MIGRATION_BASE_REF to the branch or commit used as the upgrade baseline");
}

requireSuccessful("node", ["scripts/check-migrations.mjs"]);
requireSuccessful("sqlx", ["--version"], "Install sqlx-cli 0.8.6 before running the upgrade gate.");

const temporaryRoot = mkdtempSync(resolve(tmpdir(), "layrs-migration-upgrade-"));
const baseMigrations = resolve(temporaryRoot, "migrations");
mkdirSync(baseMigrations, { recursive: true });

try {
  const listing = requireSuccessful("git", ["ls-tree", "-r", "--name-only", baseRef, migrationPath]);
  const migrationFiles = listing
    .split(/\r?\n/)
    .filter((path) => path.endsWith(".sql"))
    .sort();
  if (migrationFiles.length === 0) {
    fail(`no migrations found at ${baseRef}:${migrationPath}`);
  }

  for (const path of migrationFiles) {
    const content = requireSuccessfulBuffer("git", ["show", `${baseRef}:${path}`]);
    writeFileSync(resolve(baseMigrations, basename(path)), content);
  }

  console.log(`Layrs migration upgrade: applying ${migrationFiles.length} migration(s) from ${baseRef}.`);
  runSqlxMigrate(baseMigrations);
  console.log("Layrs migration upgrade: applying current append-only migrations.");
  runSqlxMigrate(resolve(process.cwd(), migrationPath));
  console.log("Layrs migration upgrade: baseline to current succeeded.");
} finally {
  rmSync(temporaryRoot, { recursive: true, force: true });
}

function runSqlxMigrate(source) {
  requireSuccessful("sqlx", [
    "migrate",
    "run",
    "--source",
    source,
    "--database-url",
    databaseUrl
  ]);
}

function requireSuccessful(command, args, customFailure) {
  const result = spawnSync(command, args, {
    cwd: process.cwd(),
    encoding: "utf8",
    windowsHide: true
  });
  if (result.status !== 0) {
    const detail = [result.stdout, result.stderr].filter(Boolean).join("\n").trim();
    fail(customFailure ?? `${command} ${args.join(" ")} failed:\n${detail}`);
  }
  return result.stdout.trim();
}

function requireSuccessfulBuffer(command, args) {
  const result = spawnSync(command, args, {
    cwd: process.cwd(),
    windowsHide: true
  });
  if (result.status !== 0) {
    fail(`${command} ${args.join(" ")} failed:\n${result.stderr.toString("utf8")}`);
  }
  return result.stdout;
}

function fail(message) {
  console.error(`Layrs migration upgrade failed: ${message}`);
  process.exit(1);
}
