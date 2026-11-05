import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

// Hash source bytes, including uncommitted files, while excluding build outputs.
export function sourceTreeFiles(root, inputs) {
  const files = new Map();
  function visit(relative) {
    const path = join(root, relative);
    const stat = lstatSync(path);
    if (stat.isSymbolicLink()) throw new Error(`Source symlink is not supported for distribution: ${relative}`);
    if (stat.isDirectory()) {
      for (const name of readdirSync(path).sort()) {
        if (["target", "node_modules", ".git", "dist", "gen", "build", "tmp"].includes(name)) continue;
        visit(`${relative}/${name}`);
      }
    } else if (stat.isFile()) {
      files.set(relative, createHash("sha256").update(readFileSync(path)).digest("hex"));
    }
  }
  for (const input of inputs) visit(input);
  return files;
}

export function sourceTreeDigest(root, inputs) {
  return digestFiles(sourceTreeFiles(root, inputs));
}

export function digestFiles(files) {
  const hash = createHash("sha256");
  for (const [relative, digest] of files) hash.update(`${relative}\0${digest}\n`);
  return hash.digest("hex");
}

// Name the paths a build touched so a refused package points at its cause.
export function sourceTreeChanges(before, after) {
  const changes = [];
  for (const [relative, digest] of after) {
    if (!before.has(relative)) changes.push(`added ${relative}`);
    else if (before.get(relative) !== digest) changes.push(`modified ${relative}`);
  }
  for (const relative of before.keys()) if (!after.has(relative)) changes.push(`removed ${relative}`);
  return changes;
}
