import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
const packet = JSON.parse(readFileSync(resolve("deploy/lenses/example-markdown.layrslens.json"), "utf8"));
const harness = `/@fs/${resolve("tests/e2e/fixtures/lensRuntimeHarness.tsx").replaceAll("\\", "/")}`;
async function mount(page: import("@playwright/test").Page, source?: string, tamper = false) {
  await page.goto("/");
  await page.evaluate(async ({ harness, packet, source, tamper }) => {
    const module = await import(/* @vite-ignore */ harness);
    const host = document.createElement("div"); document.body.append(host);
    await module.mountLensHarness(host, packet, source, tamper);
  }, { harness, packet, source, tamper });
}
test("downloads and runs a pinned viewer under StrictMode", async ({ page }) => {
  await mount(page);
  await expect(page.getByRole("heading", { name: "Dynamic lens" })).toBeVisible();
  await expect(page.getByText("Content from project", { exact: true })).toBeVisible();
});
test("rejects modified viewer bytes before execution", async ({ page }) => {
  await mount(page, undefined, true);
  await expect(page.getByText(/composant client n’a pas pu être vérifié/)).toBeVisible();
  await expect(page.getByText("Fallback content")).toBeVisible();
});
test("extension cannot fetch credentials or network data or access the parent DOM", async ({ page }) => {
  let leaks = 0;
  page.on("request", request => { if (request.url().includes("lens-leak")) leaks++; });
  await mount(page, `export async function render(){let blocked=false;try{await fetch('https://example.com/lens-leak')}catch{blocked=true}return {blocks:[{kind:'text',text:JSON.stringify({blocked,dom:typeof document,native:typeof __TAURI_INTERNALS__})}]}}`);
  await expect(page.getByText('{"blocked":true,"dom":"undefined","native":"undefined"}', { exact: true })).toBeVisible();
  expect(leaks).toBe(0);
});
test("terminates a runaway extension and preserves fallback", async ({ page }) => {
  await mount(page, "export function render(){while(true){}}" );
  await expect(page.getByText(/Cette lens ne peut pas afficher|lens n’a pas répondu/)).toBeVisible({timeout:12_000});
  await expect(page.getByText("Fallback content")).toBeVisible();
});
test("reuses verified project packets offline without contacting the server", async ({ page }) => {
  await mount(page);
  await expect(page.getByRole("heading", { name: "Dynamic lens" })).toBeVisible();
  await page.reload();
  await page.evaluate(async ({ harness, packet }) => {
    const module = await import(/* @vite-ignore */ harness);
    const host = document.createElement("div"); document.body.append(host);
    await module.mountLensHarness(host, packet, undefined, false, true);
  }, { harness, packet });
  await expect(page.getByRole("heading", { name: "Dynamic lens" })).toBeVisible();
  await expect(page.getByText(/Cache hors ligne/)).toBeVisible();
});

test("parallel viewers share authority reads and an older refresh cannot overwrite offline configuration", async ({ page }) => {
  await page.goto("/");
  const result = await page.evaluate(async ({ harness, packet }) => {
    const module = await import(/* @vite-ignore */ harness);
    const pin = { lensId: packet.manifest.id, version: packet.manifest.version, digest: packet.manifest.digest };
    const item = { manifest: packet.manifest, pin, source: "installed", available: true };
    const config = { revision: 1, pins: [pin], associations: [], availableCatalog: [item] };
    let calls = 0;
    const delayed: ((value: typeof config) => void)[] = [];
    let defer = false;
    const host = document.createElement("div"); document.body.append(host);
    const probe = module.mountRuntimeProbe(host, async (command: { action: string }) => {
      if (command.action === "lensPacket") return packet;
      calls++;
      return defer ? await new Promise(resolve => delayed.push(resolve)) : config;
    });
    const waitFor = async (predicate: () => boolean) => { for (let i = 0; i < 200; i++) { if (predicate()) return; await new Promise(resolve => setTimeout(resolve, 10)); } throw Error("Probe did not settle"); };
    await waitFor(() => Boolean(probe.current?.configuration));
    defer = true;
    const before = calls;
    const loads = [probe.current.load(item), probe.current.load(item)];
    await waitFor(() => delayed.length === 1);
    delayed.shift()!(config);
    const status = (await Promise.allSettled(loads)).map(value => value.status);
    const readCount = calls - before;
    const older = probe.current.refresh();
    const newer = probe.current.refresh();
    await waitFor(() => delayed.length === 2);
    delayed[1]({ ...config, revision: 2, pins: [], availableCatalog: [] }); await newer;
    delayed[0](config); await older;
    await waitFor(() => probe.current.configuration?.revision === 2);
    const cacheModule = await import(/* @vite-ignore */ harness.replace("tests/e2e/fixtures/lensRuntimeHarness.tsx", "packages/ui/src/lenses/cache.ts"));
    let saved: any;
    for (let i = 0; i < 200; i++) {
      saved = await cacheModule.readCache("runtime-probe|configuration");
      if (saved?.revision === 2) break;
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    probe.dispose();
    return { status, readCount, revision: saved?.revision, pins: saved?.pins };
  }, { harness, packet });
  expect(result).toEqual({ status: ["fulfilled", "fulfilled"], readCount: 1, revision: 2, pins: [] });
});
