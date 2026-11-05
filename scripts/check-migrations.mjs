import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const migrationDirectory = resolve(rootDir, "crates/layrs-store-server/migrations");
const manifestPath = resolve(rootDir, "scripts/migrations.lock.json");
const appendOnly = process.argv.includes("--append");

const manifest = readManifest();
const files = readMigrationFiles();
validateSequence(files);

const lockedByName = new Map(manifest.migrations.map((migration) => [migration.file, migration]));
const currentNames = new Set(files.map((migration) => migration.file));
const failures = [];

for (const locked of manifest.migrations) {
  const current = files.find((migration) => migration.file === locked.file);
  if (!current) {
    failures.push(`published migration was removed: ${locked.file}`);
  } else if (current.sha256 !== locked.sha256) {
    failures.push(
      `published migration was modified: ${locked.file}\n  expected ${locked.sha256}\n  received ${current.sha256}`
    );
  }
}

if (failures.length > 0) {
  fail(failures.join("\n"));
}

const newMigrations = files.filter((migration) => !lockedByName.has(migration.file));

if (appendOnly) {
  const highestLockedVersion = manifest.migrations.at(-1)?.version ?? 0;
  if (newMigrations.some((migration) => migration.version <= highestLockedVersion)) {
    fail("new migrations must be appended after every published migration");
  }

  if (manifest.migrations.some((migration) => !currentNames.has(migration.file))) {
    fail("the manifest cannot be updated while a published migration is missing");
  }

  if (newMigrations.length === 0) {
    console.log("Layrs migrations: manifest already includes every migration.");
    process.exit(0);
  }

  const updated = {
    ...manifest,
    migrations: [...manifest.migrations, ...newMigrations]
  };
  writeFileSync(manifestPath, `${JSON.stringify(updated, null, 2)}\n`, "utf8");
  console.log(
    `Layrs migrations: appended ${newMigrations.map((migration) => migration.file).join(", ")}.`
  );
  process.exit(0);
}

if (newMigrations.length > 0) {
  fail(
    `unlocked migration(s): ${newMigrations.map((migration) => migration.file).join(", ")}\n` +
      "Review them, then run `pnpm migrations:append`. That command never rewrites existing checksums."
  );
}

console.log(`Layrs migrations: ${files.length} immutable checksum(s) verified.`);

function readManifest() {
  if (!existsSync(manifestPath)) {
    if (!appendOnly) {
      fail("scripts/migrations.lock.json is missing; migration history is not protected");
    }
    return initialManifest();
  }

  const parsed = JSON.parse(readFileSync(manifestPath, "utf8"));
  if (parsed.schema !== 1 || !Array.isArray(parsed.migrations)) {
    fail("scripts/migrations.lock.json has an unsupported schema");
  }
  return parsed;
}

function initialManifest() {
  return {
    schema: 1,
    directory: "crates/layrs-store-server/migrations",
    hash: "sha256(utf8-lf)",
    migrations: []
  };
}

function readMigrationFiles() {
  return readdirSync(migrationDirectory, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".sql"))
    .map((entry) => {
      const match = /^(\d{4})_[a-z0-9_]+\.sql$/.exec(entry.name);
      if (!match) {
        fail(`invalid migration filename: ${entry.name}`);
      }

      const normalized = readFileSync(resolve(migrationDirectory, entry.name), "utf8").replace(
        /\r\n/g,
        "\n"
      );
      return {
        version: Number.parseInt(match[1], 10),
        file: entry.name,
        sha256: createHash("sha256").update(normalized, "utf8").digest("hex")
      };
    })
    .sort((left, right) => left.version - right.version);
}

function validateSequence(migrations) {
  for (const [index, migration] of migrations.entries()) {
    const expectedVersion = index + 1;
    if (migration.version !== expectedVersion) {
      fail(
        `migration sequence must be contiguous: expected ${String(expectedVersion).padStart(4, "0")}, found ${migration.file}`
      );
    }
  }
}

function fail(message) {
  console.error(`Layrs migration check failed:\n${message}`);
  process.exit(1);
}
