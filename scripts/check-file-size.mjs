import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

const WARN_LINES = 900;
const SOFT_LIMIT_LINES = 1000;
const HARD_LIMIT_LINES = 1500;

const generatedOrExternalPatterns = [
  /(^|[\\/])node_modules([\\/]|$)/,
  /(^|[\\/])target([\\/]|$)/,
  /(^|[\\/])\.git([\\/]|$)/,
  /(^|[\\/])\.layrs([\\/]|$)/,
  /(^|[\\/])\.layrs-local([\\/]|$)/,
  /(^|[\\/])playwright-report([\\/]|$)/,
  /(^|[\\/])test-results([\\/]|$)/,
  /(^|[\\/])tmp([\\/]|$)/,
  /(^|[\\/])Cargo\.lock$/,
  /(^|[\\/])pnpm-lock\.yaml$/,
  /(^|[\\/])apps[\\/]studio-desktop[\\/]src-tauri[\\/]gen([\\/]|$)/,
];

const sourceExtensions = new Set([
  ".rs",
  ".ts",
  ".tsx",
  ".js",
  ".jsx",
  ".mjs",
  ".cjs",
  ".css",
  ".json",
  ".md",
  ".toml",
  ".yml",
  ".yaml",
]);

const softLimitAllowlist = new Map([
  // Keep this list short and explicit. Generated/lock files belong in the
  // generatedOrExternalPatterns list above instead.
  [
    "apps/studio-desktop/src/DesktopApp.tsx",
    "desktop orchestration controller; views, settings, model helpers, and styles are split out",
  ],
  [
    "packages/client-sdk/src/normalizers.ts",
    "central wire-normalization compatibility layer; cross-coupled helpers stay together below hard limit",
  ],
  [
    "crates/layrs-client-core/src/access_registry/weaves.rs",
    "durable local weave state machine; conflict/session logic stays together below hard limit",
  ],
  [
    "crates/layrs-cli/tests/local_data_safety.rs",
    "black-box anti-loss scenarios; grouped so each workflow keeps its setup and assertions nearby",
  ],
  [
    "crates/layrs-cli/src/engine.rs",
    "CLI command facade over client-core; parser/rendering are split out and facade remains below hard limit",
  ],
  [
    "crates/layrs-client-core/src/access_registry/api_local_spaces.rs",
    "local-space public facade; storage, diff, receive, weave, and working-tree internals are split out",
  ],
  [
    "crates/layrs-lens-text/src/lib.rs",
    "built-in text Lens core; diff, preview, and reconcile must share line model below hard limit",
  ],
]);

const root = process.cwd();
const trackedFiles = execFileSync("git", ["ls-files", "--cached", "--others", "--exclude-standard"], {
  cwd: root,
  encoding: "utf8",
})
  .split(/\r?\n/)
  .filter(Boolean);

const reports = [];

for (const relativePath of trackedFiles) {
  const normalized = relativePath.replaceAll("\\", "/");
  if (generatedOrExternalPatterns.some((pattern) => pattern.test(normalized))) {
    continue;
  }

  const extension = path.extname(normalized);
  if (!sourceExtensions.has(extension)) {
    continue;
  }

  const absolutePath = path.join(root, relativePath);
  if (!existsSync(absolutePath)) {
    continue;
  }

  const contents = readFileSync(absolutePath, "utf8");
  const lines = contents.length === 0 ? 0 : contents.split(/\r?\n/).length;
  const allowedReason = softLimitAllowlist.get(normalized);

  if (lines > HARD_LIMIT_LINES) {
    reports.push({
      level: "error",
      lines,
      path: normalized,
      message: `exceeds hard limit ${HARD_LIMIT_LINES}`,
    });
  } else if (lines > SOFT_LIMIT_LINES && !allowedReason) {
    reports.push({
      level: "error",
      lines,
      path: normalized,
      message: `exceeds soft limit ${SOFT_LIMIT_LINES} without allowlist reason`,
    });
  } else if (lines > WARN_LINES) {
    reports.push({
      level: "warn",
      lines,
      path: normalized,
      message: allowedReason
        ? `above warning threshold; allowlisted: ${allowedReason}`
        : `above warning threshold ${WARN_LINES}`,
    });
  }
}

reports.sort((left, right) => right.lines - left.lines || left.path.localeCompare(right.path));

for (const report of reports) {
  const prefix = report.level === "error" ? "ERROR" : "WARN ";
  console.log(`${prefix} ${String(report.lines).padStart(5)} ${report.path} - ${report.message}`);
}

if (reports.some((report) => report.level === "error")) {
  process.exitCode = 1;
} else {
  console.log("Layrs file-size check passed.");
}
