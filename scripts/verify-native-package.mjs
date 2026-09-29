import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readdirSync } from "node:fs";
import { basename, join, resolve } from "node:path";

const directory = resolve(process.argv[2] ?? ".");
const manifest = JSON.parse(readFileSync(join(directory, "manifest.json"), "utf8"));
if (manifest.schema !== "layrs.native-distribution.v1" || !Array.isArray(manifest.files) || manifest.files.length === 0) throw new Error("Invalid package manifest");
const names = new Set(["manifest.json"]);
for (const file of manifest.files) {
  if (typeof file.path !== "string" || file.path !== basename(file.path) || /[\\/:\x00-\x1f]/.test(file.path) || [".", ".."].includes(file.path) || names.has(file.path)) throw new Error("Unsafe or duplicate package path");
  names.add(file.path);
  const path = join(directory, file.path);
  const stat = lstatSync(path);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size !== file.bytes) throw new Error(`Invalid artifact: ${file.path}`);
  if (createHash("sha256").update(readFileSync(path)).digest("hex") !== file.sha256) throw new Error(`Checksum mismatch: ${file.path}`);
}
for (const name of readdirSync(directory)) if (!names.has(name)) throw new Error(`Unexpected package entry: ${name}`);
console.log(`Verified ${manifest.files.length} artifacts; ${manifest.target}; ${manifest.profile}; signature=${manifest.signature}`);
