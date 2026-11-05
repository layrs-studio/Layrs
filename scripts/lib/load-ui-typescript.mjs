import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { webcrypto } from "node:crypto";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../packages/ui");
const ts = createRequire(path.join(root, "package.json"))("typescript");
export function loadUiTypeScript(filename) {
  const cache = new Map();
  function load(relative) {
    const resolved = path.resolve(root, "src", relative);
    if (!resolved.startsWith(path.join(root, "src") + path.sep) || !resolved.endsWith(".ts")) throw new Error("Unsupported test module");
    if (cache.has(resolved)) return cache.get(resolved);
    const module = { exports: {} };
    const code = ts.transpileModule(fs.readFileSync(resolved, "utf8"), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
    vm.runInNewContext(code, { module, exports: module.exports, URL, TextEncoder, crypto: webcrypto, require: request => {
      if (!request.startsWith("./")) throw new Error("Only local pure modules are supported");
      return load(path.relative(path.join(root, "src"), path.resolve(path.dirname(resolved), request + ".ts")));
    } });
    cache.set(resolved, module.exports); return module.exports;
  }
  return load(filename);
}
