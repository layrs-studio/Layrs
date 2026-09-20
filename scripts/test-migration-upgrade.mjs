// Compatibility command name; there is no V1/V2-to-V3 migration path.
import { spawnSync } from "node:child_process";
if (!process.env.LAYRS_TEST_DATABASE_URL || process.env.LAYRS_TEST_DATABASE_ISOLATED !== "1") {
  throw new Error("Set LAYRS_TEST_DATABASE_URL to a disposable PostgreSQL administrator endpoint and LAYRS_TEST_DATABASE_ISOLATED=1");
}
for (const [command, args] of [
  [process.execPath, ["scripts/check-migrations.mjs"]],
  ["cargo", ["test", "--locked", "-p", "layrs-server", "--test", "native_migrations", "--", "--ignored", "--nocapture"]],
]) {
  // Credentials remain in the inherited environment, never argv or diagnostics.
  const result = spawnSync(command, args, { stdio: "inherit", windowsHide: true });
  if (result.error || result.status !== 0) process.exit(result.status ?? 1);
}
