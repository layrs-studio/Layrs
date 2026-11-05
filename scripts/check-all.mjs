import { spawn } from "node:child_process";

const steps = [
  ["Migration immutability", "pnpm", ["run", "check:migrations"]],
  ["Source file size", "pnpm", ["run", "check:file-size"]],
  ["Rust formatting", "pnpm", ["run", "format:check"]],
  ["Rust Clippy", "pnpm", ["run", "lint:rust"]],
  ["TypeScript workspaces", "pnpm", ["run", "check:typescript"]],
  ["Studio production builds", "pnpm", ["run", "build:apps"]]
];

for (const [label, command, args] of steps) {
  console.log(`\nLayrs check: ${label}`);
  await run(command, args);
}

console.log("\nLayrs check: all static gates passed.");

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
