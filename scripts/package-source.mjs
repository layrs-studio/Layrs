import { createHash } from "node:crypto";
import { lstatSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

// Hash source bytes, including uncommitted files, while excluding build outputs.
export function sourceTreeDigest(root, inputs) {
  const hash = createHash("sha256");
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
      hash.update(`${relative}\0${createHash("sha256").update(readFileSync(path)).digest("hex")}\n`);
    }
  }
  for (const input of inputs) visit(input);
  return hash.digest("hex");
}
