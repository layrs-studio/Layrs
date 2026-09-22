import { spawn } from "node:child_process";

const steps = [
  {
    label: "Static trust gates",
    command: "pnpm",
    args: ["run", "check"]
  },
  {
    label: "Client SDK unit tests",
    command: "pnpm",
    args: ["run", "test:sdk"]
  },
  ...["native-project-files", "native-remote-project-files", "native-policy-path-form", "native-projection-store"].map((name) => ({
    label: name,
    command: "node",
    args: [`scripts/test-${name}.mjs`]
  })),
  {
    label: "Rust workspace with mandatory PostgreSQL",
    command: "pnpm",
    args: ["run", "test:rust"]
  },
  {
    label: "Studio Desktop Tauri shell",
    command: "pnpm",
    args: ["run", "test:tauri"]
  },
  {
    label: "Desktop and Studio Web renderer UI",
    command: "pnpm",
    args: ["run", "test:e2e:renderer"]
  },
  {
    label: "Studio Desktop native UI",
    command: "pnpm",
    args: ["run", "test:desktop:ci"]
  },
  {
    label: "Studio Web",
    command: "pnpm",
    args: ["run", "test:web:ci"]
  },
  {
    label: "Desktop to Server to Web E2E",
    command: "pnpm",
    args: ["run", "test:e2e:ci"]
  }
];

if (process.platform !== "win32" && process.env.LAYRS_ALLOW_PLATFORM_PARTIAL !== "1") {
  throw new Error(
    "The canonical `pnpm test` gate includes the native Windows Desktop suite. " +
      "Run it on Windows, or set LAYRS_ALLOW_PLATFORM_PARTIAL=1 only for a documented platform-specific CI job."
  );
}

if (process.platform !== "win32") {
  const nativeStep = steps.findIndex((step) => step.label === "Studio Desktop native UI");
  if (nativeStep >= 0) {
    steps.splice(nativeStep, 1);
  }
}

for (const step of steps) {
  console.log(`\nLayrs test: ${step.label}`);
  await run(step.command, step.args);
}

console.log("\nLayrs test: all suites passed.");

function run(command, args) {
  return new Promise((resolveCommand, rejectCommand) => {
    const child = spawn(
      process.platform === "win32" ? "cmd.exe" : command,
      process.platform === "win32" ? ["/d", "/s", "/c", command, ...args] : args,
      {
        cwd: process.cwd(),
        env: process.env,
        shell: false,
        stdio: "inherit"
      }
    );

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
