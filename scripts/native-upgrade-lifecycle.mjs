import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";

// Only the UUID-named fixture container may be stopped. Never remove its volume.
export function ensureUpgradeContainerStopped(docker, container, interrupted, spawn = spawnSync) {
  assert.match(container, /^layrs-upgrade-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
  const result = { stopped: false, stopAttempted: false, killAttempted: false };
  const invoke = (args, timeout = 10_000) => spawn("docker", [...docker, ...args], { encoding: "utf8", windowsHide: true, timeout, maxBuffer: 1024 * 1024 });
  const state = () => {
    const check = invoke(["inspect", "--format", "{{.State.Running}}", container]);
    return check.status === 0 ? check.stdout.trim() : "unknown";
  };
  let running = state();
  if (interrupted || running === "true") {
    result.stopAttempted = true;
    invoke(["stop", "--time", "10", container], 20_000);
    running = state();
    if (running === "true") {
      result.killAttempted = true;
      invoke(["kill", container]);
      running = state();
    }
  }
  result.stopped = running === "false";
  return result;
}
