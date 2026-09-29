import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

// 1000 lines is a guideline worth a look, not a rule: split files where it
// clarifies ownership, not to satisfy a counter. Only runaway files fail.
const WARN_LINES = 1000;
const HARD_LIMIT_LINES = 2000;

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

  if (lines > HARD_LIMIT_LINES) {
    reports.push({
      level: "error",
      lines,
      path: normalized,
      message: `exceeds hard limit ${HARD_LIMIT_LINES}`,
    });
  } else if (lines > WARN_LINES) {
    reports.push({
      level: "warn",
      lines,
      path: normalized,
      message: `above guideline ${WARN_LINES}; split if it clarifies ownership`,
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
