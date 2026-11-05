import { expect, test } from "@playwright/test";
import { execFileSync } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { launchNativeTauriDesktop, makeNativeSpaceFolder } from "./fixtures/nativeTauriDesktop";

function git(root: string, ...args: string[]) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
}
function commit(root: string, content: string) {
  return writeFile(join(root, "git-file.txt"), content).then(() => {
    git(root, "add", "git-file.txt");
    git(root, "-c", "user.name=Layrs Test", "-c", "user.email=test@example.invalid", "commit", "-qm", content.trim());
  });
}

test("Desktop previews Git import, rejects a changed source, then imports and exports", async ({}, testInfo) => {
  const project = await makeNativeSpaceFolder(testInfo, [{ path: "keep.txt", body: "Layrs bytes stay here\n" }]);
  const source = testInfo.outputPath("git-source");
  const destination = testInfo.outputPath("git-export");
  await mkdir(source, { recursive: true });
  git(source, "init", "-q");
  await commit(source, "first\n");
  const branch = git(source, "branch", "--show-current");
  const desktop = await launchNativeTauriDesktop(testInfo, { visual: false });
  try {
    const page = desktop.page;
    page.setDefaultTimeout(20_000);
    await page.goto(`${desktop.desktopUrl}/#/projects/new`);
    await page.getByLabel("Dossier", { exact: true }).fill(project);
    await page.getByLabel("Nom du nouveau projet").fill("Git Desktop test");
    await page.getByRole("button", { name: "Initialiser", exact: true }).click();
    await expect(page.getByRole("heading", { name: "Changements", exact: true })).toBeVisible();
    const nav = page.getByRole("complementary", { name: "Navigation principale" });
    await nav.getByRole("link", { name: "Import / export Git", exact: true }).click();
    await page.getByRole("textbox", { name: "Dossier Git source" }).fill(source);
    await page.getByRole("button", { name: "Prévisualiser" }).click();
    await expect(page.getByRole("heading", { name: "Aperçu avant confirmation" })).toBeVisible();
    await expect(page.getByText(`refs/heads/${branch}`, { exact: true })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath("git-import-preview.png"), fullPage: true });
    await commit(source, "second\n");
    await page.getByRole("button", { name: "Confirmer l’import" }).click();
    await expect(page.getByRole("alert").filter({ hasText: /changed since the preview|Git source changed/ }).first()).toBeVisible();
    await expect(page.getByRole("heading", { name: "Aperçu avant confirmation" })).toHaveCount(0);

    await page.getByRole("button", { name: "Prévisualiser" }).click();
    await expect(page.getByText("2 commit(s)", { exact: false })).toBeVisible();
    await page.getByRole("button", { name: "Confirmer l’import" }).click();
    await expect(page.getByRole("status").filter({ hasText: /branche\(s\).*importé/ }).first()).toBeVisible();
    expect(await readFile(join(project, "keep.txt"), "utf8")).toBe("Layrs bytes stay here\n");
    await nav.getByRole("link", { name: "Layers", exact: true }).click();
    const importedLayer = page.locator(".layer-list__item").filter({ has: page.locator(".layer-list__name").filter({ hasText: `git/${branch}` }) });
    await expect(importedLayer).toBeVisible();
    await importedLayer.getByRole("button", { name: "Travailler sur ce Layer" }).click();
    await expect.poll(async () => readFile(join(project, "git-file.txt"), "utf8").catch((error: NodeJS.ErrnoException) => {
      if (error.code === "ENOENT") return null;
      throw error;
    })).toBe("second\n");
    await nav.getByRole("link", { name: "Import / export Git", exact: true }).click();
    await page.getByRole("radiogroup", { name: "Opération Git" }).getByRole("radio", { name: "Exporter", exact: true }).click();
    await page.getByRole("textbox", { name: "Nouveau dossier Git" }).fill(destination);
    await page.getByRole("button", { name: "Prévisualiser" }).click();
    await expect(page.getByRole("heading", { name: "Aperçu avant confirmation" })).toBeVisible();
    await page.getByRole("button", { name: "Confirmer l’export" }).click();
    await expect(page.getByRole("status").filter({ hasText: /référence\(s\) Git exportées/ }).first()).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath("git-export-report.png"), fullPage: true });
    git(destination, "fsck", "--full", "--no-reflogs");
    expect(git(destination, "show-ref", `refs/heads/${branch}`)).toContain(`refs/heads/${branch}`);
    expect(git(destination, "show", `refs/heads/${branch}:git-file.txt`)).toBe("second");
    expect(await readFile(join(source, "git-file.txt"), "utf8")).toBe("second\n");
  } finally {
    await desktop.dispose();
  }
});
