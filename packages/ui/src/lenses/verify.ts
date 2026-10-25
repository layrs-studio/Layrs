import type { LensPacket, LensPin, LensView } from "./types";

export function canonical(value: unknown): string {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  const object = value as Record<string, unknown>;
  return `{${Object.keys(object).sort().map(key => `${JSON.stringify(key)}:${canonical(object[key])}`).join(",")}}`;
}
export async function digest(bytes: Uint8Array): Promise<string> {
  const buffer = new Uint8Array(bytes).buffer;
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", buffer));
  return `sha256:${Array.from(hash, byte => byte.toString(16).padStart(2, "0")).join("")}`;
}
export async function verifyViewer(packet: LensPacket, pin: LensPin) {
  const manifest = packet.manifest;
  if (manifest.id !== pin.lensId || manifest.version !== pin.version || manifest.digest !== pin.digest)
    throw Error("La version reçue ne correspond pas à celle du projet.");
  const identity = JSON.parse(JSON.stringify(manifest));
  delete identity.digest;
  if (identity.bundle) delete identity.bundle.signature;
  if (await digest(new TextEncoder().encode(canonical(identity))) !== pin.digest)
    throw Error("L’empreinte du manifeste est incorrecte.");
  const viewer = manifest.bundle?.entrypoints.viewer;
  if (!viewer || viewer.runtime !== "esm" || manifest.viewer.component !== "layrs.viewer.worker.v1")
    throw Error("Cette lens ne possède pas d’affichage compatible.");
  const file = packet.files.filter(file => file.path === viewer.path);
  if (file.length !== 1 || file[0].bytesBase64.length > 3 * 1024 * 1024) throw Error("Composant client absent ou trop volumineux.");
  const bytes = Uint8Array.from(atob(file[0].bytesBase64), character => character.charCodeAt(0));
  if (await digest(bytes) !== viewer.digest) throw Error("L’empreinte du composant client est incorrecte.");
  const bounded = (value: number, ceiling: number) => {
    if (!Number.isSafeInteger(value) || value < 1) throw Error("Limites du composant invalides.");
    return Math.min(value, ceiling);
  };
  return { source: new TextDecoder("utf-8", { fatal: true }).decode(bytes), exportName: viewer.export,
    limits: { input: bounded(viewer.resources.maxInputBytes, 2 * 1024 * 1024),
      output: bounded(viewer.resources.maxOutputBytes, 1024 * 1024),
      deadline: bounded(viewer.resources.maxExecutionMs, 5000) } };
}
export function validateView(value: unknown): LensView {
  if (!value || typeof value !== "object") throw Error("Affichage invalide.");
  const view = value as LensView;
  if ((view.title !== undefined && (typeof view.title !== "string" || view.title.length > 500)) || !Array.isArray(view.blocks) || view.blocks.length > 1000)
    throw Error("Affichage trop volumineux ou invalide.");
  let size = 0;
  for (const block of view.blocks) {
    if (!block || !["text", "code", "heading"].includes(block.kind) || typeof block.text !== "string") throw Error("Bloc d’affichage invalide.");
    size += block.text.length;
  }
  if (size > 1024 * 1024) throw Error("Affichage trop volumineux.");
  return view;
}
