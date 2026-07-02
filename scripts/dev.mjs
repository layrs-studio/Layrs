import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const localStateDir = resolve(rootDir, ".layrs-local");
const devEnvPath = resolve(localStateDir, "dev.env");
const cargoBin = process.platform === "win32" ? "cargo.exe" : "cargo";
const pnpmCommand = process.platform === "win32" ? "cmd.exe" : "pnpm";
const pnpmArgsPrefix = process.platform === "win32" ? ["/d", "/s", "/c", "pnpm"] : [];

const portPlans = {
  postgres: [15432, 25432, 35432, 45432, 15433, 25433],
  minioApi: [19000, 29000, 39000, 49000, 19100, 29100],
  minioConsole: [19001, 29001, 39001, 49001, 19101, 29101],
  server: [8787, 8877, 9877, 18787],
  studio: [5173, 5175, 15173, 25173]
};

const requiredDevEnvKeys = [
  "LAYRS_POSTGRES_PORT",
  "LAYRS_MINIO_API_PORT",
  "LAYRS_MINIO_CONSOLE_PORT",
  "LAYRS_DATABASE_URL",
  "LAYRS_OBJECT_STORE_ENDPOINT",
  "LAYRS_OBJECT_STORE_BUCKET",
  "LAYRS_OBJECT_STORE_ACCESS_KEY",
  "LAYRS_OBJECT_STORE_SECRET_KEY",
  "LAYRS_SERVER_ADDR",
  "LAYRS_SERVER_URL",
  "LAYRS_STUDIO_WEB_URL",
  "LAYRS_STUDIO_WEB_PORT"
];

function run(command, args) {
  return new Promise((resolveCommand, rejectCommand) => {
    const child = spawn(command, args, {
      cwd: rootDir,
      shell: false,
      stdio: "inherit"
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

function startLongProcess(name, command, args, env) {
  try {
    const child = spawn(command, args, {
      cwd: rootDir,
      shell: false,
      stdio: "inherit",
      env
    });

    child.on("error", (error) => {
      console.error(`Layrs dev: failed to start ${name}: ${command} ${args.join(" ")}`);
      console.error(`Layrs dev: ${error.message}`);
      child.emit("layrs-start-error");
    });

    return child;
  } catch (error) {
    throw new Error(`failed to start ${name}: ${command} ${args.join(" ")}: ${error.message}`);
  }
}

function canBind(port) {
  return new Promise((resolvePort) => {
    const server = createServer();

    server.once("error", () => resolvePort(false));
    server.once("listening", () => {
      server.close(() => resolvePort(true));
    });
    server.listen(port, "127.0.0.1");
  });
}

async function choosePort(name, envName, candidates) {
  const envValue = process.env[envName];
  if (envValue) {
    const port = Number.parseInt(envValue, 10);
    if (Number.isInteger(port) && port > 0 && port < 65536) {
      if (await canBind(port)) {
        return port;
      }

      throw new Error(`${envName}=${envValue} is not available on 127.0.0.1`);
    }

    throw new Error(`${envName} must be a TCP port number`);
  }

  for (const port of candidates) {
    if (await canBind(port)) {
      return port;
    }
  }

  throw new Error(`No available port found for ${name}. Tried: ${candidates.join(", ")}`);
}

function parseDevEnvFile(path) {
  const values = {};
  const content = readFileSync(path, "utf8");

  for (const line of content.split(/\r?\n/)) {
    if (!line || line.startsWith("#")) {
      continue;
    }

    const separator = line.indexOf("=");
    if (separator === -1) {
      continue;
    }

    values[line.slice(0, separator)] = line.slice(separator + 1);
  }

  return values;
}

function hasExplicitPortOverrides() {
  return [
    "LAYRS_POSTGRES_PORT",
    "LAYRS_MINIO_API_PORT",
    "LAYRS_MINIO_CONSOLE_PORT",
    "LAYRS_SERVER_PORT",
    "LAYRS_STUDIO_WEB_PORT"
  ].some((key) => Boolean(process.env[key]));
}

function readExistingDevEnvironment() {
  if (!existsSync(devEnvPath) || hasExplicitPortOverrides()) {
    return undefined;
  }

  const values = parseDevEnvFile(devEnvPath);
  if (requiredDevEnvKeys.every((key) => Boolean(values[key]))) {
    return values;
  }

  return undefined;
}

function portFromAddress(address) {
  const port = Number.parseInt(String(address).split(":").pop() ?? "", 10);
  return Number.isInteger(port) && port > 0 && port < 65536 ? port : undefined;
}

function prioritizedPorts(preferred, fallbacks) {
  return [...new Set([preferred, ...fallbacks].filter(Boolean))];
}

async function createDevEnvironment() {
  const existing = readExistingDevEnvironment();
  if (existing) {
    const serverPort = await choosePort(
      "Layrs Server",
      "LAYRS_SERVER_PORT",
      prioritizedPorts(portFromAddress(existing.LAYRS_SERVER_ADDR), portPlans.server)
    );
    const studioPort = await choosePort(
      "Studio Web",
      "LAYRS_STUDIO_WEB_PORT",
      prioritizedPorts(Number.parseInt(existing.LAYRS_STUDIO_WEB_PORT, 10), portPlans.studio)
    );
    const values = {
      ...existing,
      LAYRS_SERVER_ADDR: `127.0.0.1:${serverPort}`,
      LAYRS_SERVER_URL: `http://127.0.0.1:${serverPort}`,
      LAYRS_STUDIO_WEB_URL: `http://127.0.0.1:${studioPort}`,
      LAYRS_STUDIO_WEB_PORT: String(studioPort)
    };

    writeDevEnvironment(values);
    return values;
  }

  const postgresPort = await choosePort("PostgreSQL", "LAYRS_POSTGRES_PORT", portPlans.postgres);
  const minioApiPort = await choosePort("MinIO API", "LAYRS_MINIO_API_PORT", portPlans.minioApi);
  const minioConsolePort = await choosePort(
    "MinIO console",
    "LAYRS_MINIO_CONSOLE_PORT",
    portPlans.minioConsole
  );
  const serverPort = await choosePort("Layrs Server", "LAYRS_SERVER_PORT", portPlans.server);
  const studioPort = await choosePort("Studio Web", "LAYRS_STUDIO_WEB_PORT", portPlans.studio);

  const values = {
    LAYRS_POSTGRES_PORT: String(postgresPort),
    LAYRS_MINIO_API_PORT: String(minioApiPort),
    LAYRS_MINIO_CONSOLE_PORT: String(minioConsolePort),
    LAYRS_DATABASE_URL: `postgres://layrs:layrs@127.0.0.1:${postgresPort}/layrs`,
    LAYRS_OBJECT_STORE_ENDPOINT: `http://127.0.0.1:${minioApiPort}`,
    LAYRS_OBJECT_STORE_BUCKET: "layrs-dev",
    LAYRS_OBJECT_STORE_ACCESS_KEY: "layrs",
    LAYRS_OBJECT_STORE_SECRET_KEY: "layrs-local-secret",
    LAYRS_SERVER_ADDR: `127.0.0.1:${serverPort}`,
    LAYRS_SERVER_URL: `http://127.0.0.1:${serverPort}`,
    LAYRS_STUDIO_WEB_URL: `http://127.0.0.1:${studioPort}`,
    LAYRS_STUDIO_WEB_PORT: String(studioPort)
  };

  writeDevEnvironment(values);

  return values;
}

function writeDevEnvironment(values) {
  mkdirSync(localStateDir, { recursive: true });
  writeFileSync(
    devEnvPath,
    `${Object.entries(values)
      .map(([key, value]) => `${key}=${value}`)
      .join("\n")}\n`,
    "utf8"
  );
}

async function main() {
  if (!existsSync(resolve(rootDir, "node_modules"))) {
    console.warn("Layrs dev: node_modules is missing. Run `pnpm install` once before Studio can start.");
  }

  const devEnv = await createDevEnvironment();

  console.log("Layrs dev: starting Docker services...");
  await run("docker", ["compose", "--env-file", devEnvPath, "up", "-d", "--remove-orphans"]);

  console.log(`Layrs dev: starting Layrs Server at ${devEnv.LAYRS_SERVER_URL}`);
  if (process.env.LAYRS_DEV_SKIP_STUDIO !== "1") {
    console.log(`Layrs dev: starting Studio Web at ${devEnv.LAYRS_STUDIO_WEB_URL}`);
  }
  console.log(`Layrs dev: PostgreSQL on 127.0.0.1:${devEnv.LAYRS_POSTGRES_PORT}`);
  console.log(`Layrs dev: MinIO API on ${devEnv.LAYRS_OBJECT_STORE_ENDPOINT}`);
  console.log("Layrs dev: Docker services keep running until `pnpm run dev:down`.");

  const serverPort = portFromAddress(devEnv.LAYRS_SERVER_ADDR);
  const serverTargetDir = `target/dev-server-${serverPort ?? "default"}`;

  let shuttingDown = false;
  const children = [];
  const shutdown = (exitCode = 0) => {
    if (shuttingDown) {
      return;
    }

    shuttingDown = true;
    for (const child of children) {
      child.kill();
    }
    process.exitCode = exitCode;
  };

  const server = startLongProcess(
    "Layrs Server",
    cargoBin,
    ["run", "--target-dir", serverTargetDir, "-p", "layrs-server"],
    {
      ...process.env,
      ...devEnv
    }
  );
  children.push(server);

  if (process.env.LAYRS_DEV_SKIP_STUDIO !== "1") {
    try {
      const studio = startLongProcess(
        "Studio Web",
        pnpmCommand,
        [
          ...pnpmArgsPrefix,
          "--filter",
          "@layrs/studio-web",
          "dev",
          "--",
          "--port",
          devEnv.LAYRS_STUDIO_WEB_PORT,
          "--strictPort"
        ],
        {
          ...process.env,
          ...devEnv,
          VITE_LAYRS_API_URL: devEnv.LAYRS_SERVER_URL,
          VITE_LAYRS_SERVER_URL: devEnv.LAYRS_SERVER_URL
        }
      );
      children.push(studio);
    } catch (error) {
      shutdown(1);
      throw error;
    }
  }

  process.on("SIGINT", () => shutdown(0));
  process.on("SIGTERM", () => shutdown(0));

  server.on("layrs-start-error", () => shutdown(1));

  for (const child of children) {
    child.on("exit", (code, signal) => {
      if (shuttingDown) {
        return;
      }

      const exitCode = code ?? (signal ? 1 : 0);
      shutdown(exitCode);
    });
  }
}

main().catch((error) => {
  console.error(`Layrs dev: ${error.message}`);
  process.exit(1);
});
