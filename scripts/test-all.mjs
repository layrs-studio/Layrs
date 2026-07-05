import { spawn } from "node:child_process";

const steps = [
  {
    label: "Core and CLI",
    command: "pnpm",
    args: ["run", "test:core"]
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
