import assert from "node:assert/strict";
import test from "node:test";
import { ensureUpgradeContainerStopped } from "./native-upgrade-lifecycle.mjs";

const name = "layrs-upgrade-12345678-1234-1234-1234-123456789abc";
function fixture(states) {
  const calls = [];
  const spawn = (command, args) => {
    assert.equal(command, "docker"); calls.push(args);
    assert.equal(args.at(-1), name);
    if (args[0] === "inspect") return { status: 0, stdout: states.shift() };
    return { status: 0, stdout: "" };
  };
  return { calls, spawn };
}
test("completed container is checked and retained without stop or deletion", () => {
  const f = fixture(["false"]);
  assert.deepEqual(ensureUpgradeContainerStopped([], name, false, f.spawn), { stopped: true, stopAttempted: false, killAttempted: false });
  assert.equal(f.calls.length, 1);
});
test("timeout stops only its fixture before export can be allowed", () => {
  const f = fixture(["true", "false"]);
  assert.deepEqual(ensureUpgradeContainerStopped([], name, true, f.spawn), { stopped: true, stopAttempted: true, killAttempted: false });
  assert.deepEqual(f.calls[1], ["stop", "--time", "10", name]);
});
test("failed graceful stop kills the same fixture and verifies termination", () => {
  const f = fixture(["true", "true", "false"]);
  assert.deepEqual(ensureUpgradeContainerStopped([], name, true, f.spawn), { stopped: true, stopAttempted: true, killAttempted: true });
  assert.deepEqual(f.calls[3], ["kill", name]);
});
test("unknown or still running container never permits export; foreign names reject", () => {
  for (const states of [["unknown", "unknown"], ["true", "true", "true"]]) {
    const f = fixture(states);
    assert.equal(ensureUpgradeContainerStopped([], name, true, f.spawn).stopped, false);
  }
  assert.throws(() => ensureUpgradeContainerStopped([], "postgres", true), /assert|match/i);
});
