import { spawn } from "node:child_process";

const steps = [
  {
    label: "CLI",
    command: "cargo",
    args: ["test", "-p", "layrs-cli"],
    env: {}
  },
  {
    label: "Client Core",
    command: "cargo",
    args: ["test", "-p", "layrs-client-core", "--", "--test-threads=1"],
    env: {
      RUST_TEST_THREADS: "1"
    }
  }
];

for (const step of steps) {
  console.log(`\nLayrs core test: ${step.label}`);
  await run(step.command, step.args, step.env);
}

console.log("\nLayrs core test: all suites passed.");

function run(command, args, extraEnv) {
  return new Promise((resolveCommand, rejectCommand) => {
    const child = spawn(
      process.platform === "win32" ? "cmd.exe" : command,
      process.platform === "win32" ? ["/d", "/s", "/c", command, ...args] : args,
      {
        cwd: process.cwd(),
        env: {
          ...process.env,
          ...extraEnv
        },
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
