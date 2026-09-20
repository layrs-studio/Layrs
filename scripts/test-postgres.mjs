import { randomUUID } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";

const scope = process.argv.includes("--server") ? "server" : "workspace";
const externalDatabaseUrl = process.env.LAYRS_TEST_DATABASE_URL;
const postgresImage = process.env.LAYRS_TEST_POSTGRES_IMAGE ?? "postgres:16.14-alpine3.23";
const containerName = `layrs-test-postgres-${process.pid}-${randomUUID().slice(0, 8)}`;
let ownedContainer = false;

installSignalHandler("SIGINT", 130);
installSignalHandler("SIGTERM", 143);

try {
  const databaseUrl = externalDatabaseUrl ?? startEphemeralPostgres();
  if (externalDatabaseUrl && process.env.LAYRS_TEST_DATABASE_ISOLATED !== "1") {
    throw new Error(
      "LAYRS_TEST_DATABASE_URL requires LAYRS_TEST_DATABASE_ISOLATED=1. " +
        "Use a disposable database; the server tests migrate and mutate it."
    );
  }

  console.log(`Layrs PostgreSQL test: using ${redactDatabaseUrl(databaseUrl)}`);
  console.log("Layrs PostgreSQL test: database-backed tests are mandatory for this phase.");

  const cargoArgs =
    scope === "server"
      ? ["test", "-p", "layrs-server", "--", "--test-threads=1"]
      : ["test", "--workspace", "--", "--test-threads=1"];

  const environment = {
    ...process.env,
    DATABASE_URL: databaseUrl,
    LAYRS_TEST_DATABASE_URL: databaseUrl,
    LAYRS_TEST_DATABASE_ISOLATED: "1",
    LAYRS_REQUIRE_TEST_DATABASE: "1",
    RUST_TEST_THREADS: "1"
  };
  await run("cargo", cargoArgs, environment);
  await run(process.execPath, ["scripts/test-native-postgres.mjs"], environment);
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`Layrs PostgreSQL test failed: ${message}`);
  process.exitCode = 1;
} finally {
  cleanup();
}

function startEphemeralPostgres() {
  requireSuccessful("docker", ["info", "--format", "{{.ServerVersion}}"], {
    failureMessage:
      "Docker is required when LAYRS_TEST_DATABASE_URL is not provided. " +
      "Start Docker or provide an explicitly isolated PostgreSQL URL."
  });

  console.log(`Layrs PostgreSQL test: starting disposable ${postgresImage}...`);
  requireSuccessful("docker", [
    "run",
    "--rm",
    "--detach",
    "--name",
    containerName,
    "--env",
    "POSTGRES_DB=layrs_test",
    "--env",
    "POSTGRES_USER=layrs",
    "--env",
    "POSTGRES_PASSWORD=layrs",
    "--publish",
    "127.0.0.1::5432",
    postgresImage
  ]);
  ownedContainer = true;

  waitForPostgres();
  const portOutput = requireSuccessful("docker", ["port", containerName, "5432/tcp"]);
  const match = /127\.0\.0\.1:(\d+)/.exec(portOutput);
  if (!match) {
    throw new Error(`could not determine the disposable PostgreSQL port from: ${portOutput}`);
  }

  return `postgres://layrs:layrs@127.0.0.1:${match[1]}/layrs_test`;
}

function waitForPostgres() {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    const result = spawnSync(
      "docker",
      ["exec", containerName, "pg_isready", "-U", "layrs", "-d", "layrs_test"],
      { encoding: "utf8", windowsHide: true }
    );
    if (result.status === 0) {
      return;
    }
    sleep(500);
  }
  throw new Error("disposable PostgreSQL did not become ready within 60 seconds");
}

function run(command, args, env) {
  return new Promise((resolveCommand, rejectCommand) => {
    const child = spawn(command, args, {
      cwd: process.cwd(),
      env,
      shell: false,
      stdio: "inherit",
      windowsHide: true
    });

    child.on("error", rejectCommand);
    child.on("exit", (code, signal) => {
      if (code === 0) {
        resolveCommand();
        return;
      }
      rejectCommand(new Error(`${command} ${args.join(" ")} exited with ${signal ?? code}`));
    });
  });
}

function requireSuccessful(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: process.cwd(),
    encoding: "utf8",
    windowsHide: true
  });
  if (result.status !== 0) {
    const detail = [result.stdout, result.stderr].filter(Boolean).join("\n").trim();
    throw new Error(options.failureMessage ?? `${command} ${args.join(" ")} failed:\n${detail}`);
  }
  return result.stdout.trim();
}

function redactDatabaseUrl(value) {
  try {
    const parsed = new URL(value);
    if (parsed.password) {
      parsed.password = "***";
    }
    return parsed.toString();
  } catch {
    return "<configured database URL>";
  }
}

function cleanup() {
  if (!ownedContainer) {
    return;
  }
  spawnSync("docker", ["rm", "--force", containerName], {
    stdio: "ignore",
    windowsHide: true
  });
  ownedContainer = false;
}

function installSignalHandler(signal, exitCode) {
  process.on(signal, () => {
    cleanup();
    process.exit(exitCode);
  });
}

function sleep(milliseconds) {
  const buffer = new SharedArrayBuffer(4);
  Atomics.wait(new Int32Array(buffer), 0, 0, milliseconds);
}
