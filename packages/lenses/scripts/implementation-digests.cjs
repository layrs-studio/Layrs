"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const BUILTIN_LENS_IDS = ["layrs.code", "layrs.image", "layrs.text", "layrs.raw"];
const IMPLEMENTATION_DOMAIN = "layrs.lenses.implementation.v1\0";
const SOURCE_EXTENSIONS = new Set([".css", ".ts", ".tsx"]);

function readImplementationEntries() {
  const sourceRoots = [
    { prefix: "packages/lenses/src", root: path.resolve(__dirname, "../src") },
    { prefix: "packages/lens-sdk/src", root: path.resolve(__dirname, "../../lens-sdk/src") }
  ];
  return sourceRoots
    .flatMap(({ prefix, root }) =>
      walk(root)
        .filter((absolutePath) => SOURCE_EXTENSIONS.has(path.extname(absolutePath)))
        .map((absolutePath) => ({
          path: `${prefix}/${path.relative(root, absolutePath).replaceAll("\\", "/")}`,
          bytes: fs.readFileSync(absolutePath)
        }))
    )
    .filter((entry) => entry.path !== "packages/lenses/src/manifests.ts")
    .sort((left, right) => left.path.localeCompare(right.path));
}

function implementationDigest(lensId, entries) {
  const hash = crypto.createHash("sha256");
  hash.update(IMPLEMENTATION_DOMAIN);
  hash.update(lensId);
  hash.update("\0");
  for (const entry of entries) {
    const pathBytes = Buffer.from(entry.path, "utf8");
    hash.update(u64(pathBytes.length));
    hash.update(pathBytes);
    hash.update(u64(entry.bytes.length));
    hash.update(entry.bytes);
  }
  return `sha256:${hash.digest("hex")}`;
}

function builtinImplementationDigests(entries = readImplementationEntries()) {
  return Object.fromEntries(
    BUILTIN_LENS_IDS.map((lensId) => [lensId, implementationDigest(lensId, entries)])
  );
}

function walk(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const absolutePath = path.join(directory, entry.name);
    return entry.isDirectory() ? walk(absolutePath) : [absolutePath];
  });
}

function u64(value) {
  const buffer = Buffer.allocUnsafe(8);
  buffer.writeBigUInt64BE(BigInt(value));
  return buffer;
}

if (require.main === module) {
  process.stdout.write(`${JSON.stringify(builtinImplementationDigests(), null, 2)}\n`);
}

module.exports = {
  BUILTIN_LENS_IDS,
  builtinImplementationDigests,
  implementationDigest,
  readImplementationEntries
};
