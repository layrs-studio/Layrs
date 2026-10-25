import { StrictMode, useEffect } from "react";
import { createRoot } from "react-dom/client";
import { ProjectLensProvider, useLensRuntime } from "../../../packages/ui/src/lenses/runtime";
import { LensViewer } from "../../../packages/ui/src/lenses/LensViewer";
import { canonical, digest } from "../../../packages/ui/src/lenses/verify";
import type { LensPacket, LensTransport } from "../../../packages/ui/src/lenses/types";

export async function mountLensHarness(host: HTMLElement, packet: LensPacket, source?: string, tamper = false, offline = false) {
  packet = structuredClone(packet);
  if (source !== undefined) {
    packet.files[0].bytesBase64 = btoa(source);
    packet.manifest.bundle!.entrypoints.viewer!.digest = await digest(new TextEncoder().encode(source));
    delete packet.manifest.digest;
    packet.manifest.digest = await digest(new TextEncoder().encode(canonical(packet.manifest)));
  }
  const pin = { lensId: packet.manifest.id, version: packet.manifest.version, digest: packet.manifest.digest! };
  if (tamper) packet.files[0].bytesBase64 = btoa("export function render(){return {blocks:[]}}");
  const item = { manifest: packet.manifest, pin, source: "installed", available: true };
  const transport: LensTransport = async <T,>(command: { action: string }) => {
    if (offline) throw Error("Network unavailable");
    if (command.action === "projectLenses") return { revision: 1, pins: [pin], associations: [{ extension: "md", lensId: pin.lensId }], availableCatalog: [item] } as T;
    if (command.action === "lensPacket") return packet as T;
    throw Error("Unexpected command");
  };
  const root = createRoot(host);
  root.render(<StrictMode><ProjectLensProvider scope="test-project" transport={transport} offline={offline} persist><LensViewer input={{kind:"preview",path:"readme.md",text:"# Dynamic lens\nContent from project"}} fallback={<p>Fallback content</p>} /></ProjectLensProvider></StrictMode>);
  return { dispose: () => root.unmount() };
}

export function mountRuntimeProbe(host: HTMLElement, transport: LensTransport, scope = "runtime-probe") {
  const result: { current: ReturnType<typeof useLensRuntime>; dispose: () => void } = { current: null, dispose: () => root.unmount() };
  function Probe() {
    const runtime = useLensRuntime();
    useEffect(() => { result.current = runtime; }, [runtime]);
    return <p>Revision {runtime?.configuration?.revision ?? "pending"}</p>;
  }
  const root = createRoot(host);
  root.render(<ProjectLensProvider scope={scope} transport={transport} persist><Probe /></ProjectLensProvider>);
  return result;
}
