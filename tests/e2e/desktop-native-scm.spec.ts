import { expect,test,type Page } from "@playwright/test";
import { readFile,writeFile,rename } from "node:fs/promises";
import { resolve } from "node:path";
import { launchNativeTauriDesktop,makeNativeSpaceFolder } from "./fixtures/nativeTauriDesktop";

// A guarded materialization can briefly remove the old leaf before installing
// its replacement. Retry that transient absence; preserve all other I/O errors.
async function projectedText(path: string) {
  try { return await readFile(path,"utf8"); }
  catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") return null; throw error; }
}

const nav=(page:Page,label:string)=>page.getByRole("complementary",{name:"Navigation principale"}).getByRole("link",{name:label,exact:true});
async function open(page:Page,label:string,heading=label){
  await nav(page,label).click();
  await expect(page.getByRole("heading",{name:heading,exact:true})).toBeVisible();
}
async function createProject(page:Page,url:string,root:string,name:string){
  await page.goto(`${url}/#/projects/new`);
  await page.getByLabel("Dossier",{exact:true}).fill(root);
  await page.getByLabel("Nom du nouveau projet").fill(name);
  await page.getByRole("button",{name:"Initialiser",exact:true}).click();
  await expect(page.getByRole("heading",{name:"Changements",exact:true})).toBeVisible();
  await expect(page.getByRole("complementary",{name:"Navigation principale"}).getByText(name,{exact:true}).first()).toBeVisible();
}
async function automaticCapture(page:Page,on:boolean){
  const control=page.getByRole("switch",{name:"Capture automatique",exact:true});
  if((await control.getAttribute("aria-checked"))!==String(on))await control.click();
  await expect(control).toHaveAttribute("aria-checked",String(on));
}
async function newLayer(page:Page,name:string){
  await open(page,"Layers");
  await page.getByRole("button",{name:"Nouveau Layer",exact:true}).click();
  await page.getByRole("dialog").getByLabel("Nom",{exact:true}).fill(name);
  await page.getByRole("button",{name:"Créer depuis le travail courant",exact:true}).click();
  await expect(page.getByRole("combobox",{name:"Layer",exact:true}).locator("option").filter({hasText:new RegExp(`^${name}$`)})).toHaveCount(1);
}
/** Captures once the edited path is listed, as a person would. */
async function capture(page:Page,path:string){
  await open(page,"Changements");
  await expect(page.getByRole("region",{name:"Fichiers modifiés"}).getByText(path,{exact:false}).first()).toBeVisible({timeout:15000});
  // Keyboard activation: a confirmation toast may still cover the button.
  await page.getByRole("button",{name:"Créer une capture",exact:true}).press("Enter");
  await expect(page.getByText("Aucune modification de fichier.",{exact:true})).toBeVisible();
}
async function switchLayer(page:Page,label:string){
  await page.getByRole("combobox",{name:"Layer",exact:true}).selectOption({label});
  await expect(page.getByRole("combobox",{name:"Layer",exact:true}).locator("option:checked")).toHaveText(label);
}
/** From Layers: merge `source` into the current Layer through the merge workspace. */
async function previewMerge(page:Page,source:string){
  await open(page,"Layers");
  await page.locator(".layer-list__item").filter({hasText:source}).getByRole("button",{name:/^Fusionner dans /}).click();
  await expect(page.getByRole("heading",{name:"Préparer une fusion",exact:true})).toBeVisible();
  await expect(page.getByRole("combobox",{name:"Source de la fusion",exact:true})).toHaveValue(source);
  await page.getByRole("button",{name:"Prévisualiser la fusion",exact:true}).click();
}
async function applyPreview(page:Page,remaining:string){
  const preview=page.getByRole("region",{name:"Aperçu de fusion",exact:true});
  await expect(preview.getByText(remaining,{exact:true})).toBeVisible();
  await preview.getByRole("button",{name:"Appliquer cet aperçu",exact:true}).click();
  await expect(preview).toHaveCount(0);
}
async function undo(page:Page){
  await open(page,"Historique");
  await page.getByRole("button",{name:"Annuler",exact:true}).click();
}
async function openMerge(page:Page,url:string,root:string){
  await page.goto(`${url}/#/projects/${encodeURIComponent(root)}/merge`);
  await expect(page.getByRole("heading",{name:"Fusion",exact:true})).toBeVisible();
}

test("native Desktop resolves all paths of a rename conflict together",async ({},testInfo)=>{
  const root=await makeNativeSpaceFolder(testInfo,[{path:"old.txt",body:"base\n"}]);
  const desktop=await launchNativeTauriDesktop(testInfo,{visual:false});
  try {
    const page=desktop.page;
    page.setDefaultTimeout(20_000);
    await createProject(page,desktop.desktopUrl,root,"Renommage test");
    await automaticCapture(page,false);
    await newLayer(page,"topic");
    await writeFile(resolve(root,"old.txt"),"modified\n","utf8");
    await capture(page,"old.txt");
    await switchLayer(page,"topic");
    await expect.poll(()=>projectedText(resolve(root,"old.txt"))).toBe("base\n");
    await rename(resolve(root,"old.txt"),resolve(root,"new.txt"));
    await capture(page,"new.txt");
    await switchLayer(page,"main");
    await expect.poll(()=>projectedText(resolve(root,"old.txt"))).toBe("modified\n");
    await previewMerge(page,"topic");
    await applyPreview(page,"1 conflit restera après cette opération.");
    const conflicts=page.getByRole("navigation",{name:"Conflits",exact:true});
    await expect(conflicts.getByRole("link")).toHaveCount(1);
    await expect(conflicts.getByText("Chemins liés par un renommage",{exact:true})).toBeVisible();
    const merge=page.getByRole("region",{name:"Conflit sélectionné",exact:true});
    await expect(merge.getByText("Ce conflit concerne plusieurs chemins liés par un renommage.",{exact:true})).toBeVisible();
    await expect(merge.getByRole("region",{name:"Comparaison des variantes",exact:true})).toHaveCount(0);
    await expect(merge.getByRole("region",{name:"Texte final",exact:true})).toHaveCount(0);
    const variants=merge.getByRole("listitem",{name:/^Variante \d$/});
    await expect(variants).toHaveCount(2);
    await expect(variants.first().getByRole("button",{name:"Lire le contenu",exact:true})).toHaveCount(0);
    await variants.first().getByRole("button",{name:"Prévisualiser cette variante",exact:true}).click();
    await applyPreview(page,"Aucun conflit ne restera après cette opération.");
    await expect(page.getByRole("heading",{name:"Préparer une fusion",exact:true})).toBeVisible();
    const paths=await Promise.all([projectedText(resolve(root,"old.txt")),projectedText(resolve(root,"new.txt"))]);
    expect([["modified\n",null],[null,"base\n"]]).toContainEqual(paths);
    await undo(page);
    await expect.poll(()=>projectedText(resolve(root,"old.txt"))).toBe("modified\n");
    await openMerge(page,desktop.desktopUrl,root);
    await expect(page.getByRole("region",{name:"Conflit sélectionné",exact:true}).getByRole("listitem",{name:/^Variante \d$/})).toHaveCount(2);
    await page.screenshot({path:testInfo.outputPath("native-rename-conflict.png"),fullPage:true});
  } finally {await desktop.dispose();}
});

test("native Desktop preserves block drafts through partial resolution and reopen",async ({},testInfo)=>{
  const base="zero\r\nsame\r\nzero\r\n";
  const current="B\r\nsame\r\nB\r\n";
  const root=await makeNativeSpaceFolder(testInfo,[{path:"code.ts",body:base}]);
  const desktop=await launchNativeTauriDesktop(testInfo,{visual:false});
  try {
    const page=desktop.page;
    page.setDefaultTimeout(20_000);
    await createProject(page,desktop.desktopUrl,root,"Blocs test");
    await automaticCapture(page,false);
    await newLayer(page,"topic");
    await writeFile(resolve(root,"code.ts"),current,"utf8");
    await capture(page,"code.ts");
    await switchLayer(page,"topic");
    await expect.poll(()=>projectedText(resolve(root,"code.ts"))).toBe(base);
    await writeFile(resolve(root,"code.ts"),"é\r\nsame\r\né\r\n","utf8");
    await capture(page,"code.ts");
    await switchLayer(page,"main");
    await expect.poll(()=>projectedText(resolve(root,"code.ts"))).toBe(current);
    await previewMerge(page,"topic");
    await applyPreview(page,"1 conflit restera après cette opération.");
    const blocks=page.getByRole("region",{name:"Résolution par passages",exact:true});
    await expect(blocks.getByText("2 passages à résoudre.",{exact:false})).toBeVisible();
    await blocks.getByRole("button",{name:"Passage 2 À résoudre",exact:true}).click();
    await blocks.getByRole("textbox",{name:"Texte du passage 2",exact:true}).fill("résolution manuelle\n");
    await blocks.getByRole("button",{name:"Passage 1 À résoudre",exact:true}).click();
    const firstBase=blocks.getByRole("listitem",{name:"Base du passage 1",exact:true});
    await firstBase.getByRole("button",{name:"Lire ce passage",exact:true}).click();
    await expect(firstBase.locator("pre")).toContainText("zero");
    await firstBase.getByRole("button",{name:"Prévisualiser ce choix",exact:true}).click();
    await applyPreview(page,"1 conflit restera après cette opération.");
    expect(await readFile(resolve(root,"code.ts"),"utf8")).toBe(current);
    await expect(blocks.getByRole("button",{name:"Passage 1 Résolu",exact:true})).toBeVisible();
    const second=blocks.getByRole("textbox",{name:"Texte du passage 2",exact:true});
    if(!(await second.isVisible()))await blocks.getByRole("button",{name:"Passage 2 À résoudre",exact:true}).click();
    await expect(second).toHaveValue("résolution manuelle\n");
    await blocks.getByRole("button",{name:"Prévisualiser le texte de ce passage",exact:true}).click();
    await applyPreview(page,"Aucun conflit ne restera après cette opération.");
    await expect.poll(()=>projectedText(resolve(root,"code.ts"))).toBe("zero\r\nsame\r\nrésolution manuelle\n");
    await undo(page);
    await openMerge(page,desktop.desktopUrl,root);
    await expect(page.getByRole("region",{name:"Résolution par passages",exact:true}).getByRole("button",{name:"Passage 1 Résolu",exact:true})).toBeVisible();
    await page.screenshot({path:testInfo.outputPath("native-partial-blocks.png"),fullPage:true});
  } finally {await desktop.dispose();}
});

test("native Desktop previews a conflict and installs a chosen canonical variant",async ({},testInfo)=>{
  const root=await makeNativeSpaceFolder(testInfo,[{path:"code.ts",body:"const value = \"base\";\n"}]);
  const desktop=await launchNativeTauriDesktop(testInfo,{visual:false});
  try {
    const page=desktop.page;
    page.setDefaultTimeout(20_000);
    await createProject(page,desktop.desktopUrl,root,"Fusion test");
    await automaticCapture(page,false);
    await newLayer(page,"topic");
    await writeFile(resolve(root,"code.ts"),"const value = \"main\";\n","utf8");
    await capture(page,"code.ts");
    await switchLayer(page,"topic");
    await expect.poll(()=>projectedText(resolve(root,"code.ts"))).toBe("const value = \"base\";\n");
    await writeFile(resolve(root,"code.ts"),"const value = \"topic\";\n","utf8");
    await capture(page,"code.ts");
    await switchLayer(page,"main");
    await expect.poll(()=>projectedText(resolve(root,"code.ts"))).toBe("const value = \"main\";\n");
    await expect(page.getByRole("combobox",{name:"Layer",exact:true})).toBeEnabled();
    await previewMerge(page,"topic");
    await applyPreview(page,"1 conflit restera après cette opération.");
    const merge=page.getByRole("region",{name:"Conflit sélectionné",exact:true});
    const compared=merge.getByRole("region",{name:"Comparaison des variantes",exact:true});
    await expect(compared.locator("td").filter({hasText:'"base"'}).first()).toBeVisible();
    expect(await readFile(resolve(root,"code.ts"),"utf8")).toBe("const value = \"main\";\n");
    const variant=merge.getByRole("listitem",{name:"Variante 1",exact:true});
    await variant.getByRole("button",{name:"Lire le contenu",exact:true}).click();
    await expect(variant.getByRole("button",{name:"Utiliser comme texte final",exact:true})).toBeVisible();
    await variant.getByRole("button",{name:"Prévisualiser cette variante",exact:true}).click();
    await applyPreview(page,"Aucun conflit ne restera après cette opération.");
    await expect(page.getByRole("heading",{name:"Préparer une fusion",exact:true})).toBeVisible();
    expect(["const value = \"main\";\n","const value = \"topic\";\n"]).toContain(await readFile(resolve(root,"code.ts"),"utf8"));
    await undo(page);
    await openMerge(page,desktop.desktopUrl,root);
    const text=page.getByRole("region",{name:"Texte final",exact:true});
    await text.getByLabel("Contenu final",{exact:true}).fill("const value = \"combined\";\n");
    await text.getByRole("button",{name:"Prévisualiser le texte final",exact:true}).click();
    await applyPreview(page,"Aucun conflit ne restera après cette opération.");
    await expect.poll(()=>projectedText(resolve(root,"code.ts"))).toBe("const value = \"combined\";\n");
    await expect(page.getByRole("heading",{name:"Préparer une fusion",exact:true})).toBeVisible();
    await page.screenshot({path:testInfo.outputPath("native-merge.png"),fullPage:true});
  }finally{await desktop.dispose();}
});

test("native SCM Desktop captures, previews and restores through real IPC",async ({},testInfo)=>{
  const root=await makeNativeSpaceFolder(testInfo,[{path:"readme.txt",body:"Initial contents\n"}]);
  const desktop=await launchNativeTauriDesktop(testInfo,{visual:false});
  try {
    const page=desktop.page;
    page.setDefaultTimeout(20_000);
    await createProject(page,desktop.desktopUrl,root,"Native SCM desktop");
    await open(page,"Réglages du projet");
    const identity=page.getByRole("region",{name:"Identité des prochaines captures",exact:true});
    await identity.getByLabel("Nom affiché",{exact:true}).fill("Desktop Alice");
    await identity.getByRole("button",{name:"Enregistrer l’identité locale",exact:true}).click();
    await expect(identity.locator(".layrs-technical-id code")).toHaveText(/^actor:/);
    const actor=await identity.locator(".layrs-technical-id code").innerText();
    await open(page,"Changements");
    await automaticCapture(page,false);
    await open(page,"Versions","Versions et repères");
    await page.getByRole("button",{name:"Nouveau repère",exact:true}).click();
    await page.getByLabel("Nom ou version",{exact:true}).fill("baseline");
    await page.getByRole("button",{name:"Créer un marker",exact:true}).click();
    await expect(page.getByRole("heading",{name:"baseline",exact:true})).toBeVisible();
    await writeFile(resolve(root,"readme.txt"),"Changed contents\n","utf8");
    await capture(page,"readme.txt");
    await open(page,"Versions","Versions et repères");
    const marker=page.locator(".points__item").filter({has:page.getByRole("heading",{name:"baseline",exact:true})});
    await marker.getByRole("button",{name:"Prévisualiser",exact:true}).click();
    const preview=page.getByRole("region",{name:"Prévisualisation de restauration"});
    await expect(preview).toBeVisible();
    await expect(preview.getByText("readme.txt",{exact:false})).toBeVisible();
    await page.getByRole("dialog").getByRole("button",{name:"Appliquer cet aperçu",exact:true}).click();
    await expect.poll(()=>projectedText(resolve(root,"readme.txt"))).toBe("Initial contents\n");
    await undo(page);
    await expect.poll(()=>projectedText(resolve(root,"readme.txt"))).toBe("Changed contents\n");
    await page.getByRole("button",{name:"Rétablir",exact:true}).click();
    await expect.poll(()=>projectedText(resolve(root,"readme.txt"))).toBe("Initial contents\n");
    await open(page,"Versions","Versions et repères");
    await page.getByRole("button",{name:"Nouveau repère",exact:true}).click();
    await page.getByLabel("Nom ou version",{exact:true}).fill("restored");
    await page.getByRole("button",{name:"Créer un marker",exact:true}).click();
    await expect(page.getByRole("heading",{name:"restored",exact:true})).toBeVisible();
    await open(page,"Réglages du projet");
    await page.getByRole("combobox",{name:"Action",exact:true}).selectOption("revert");
    await page.getByLabel("Capture ou référence",{exact:true}).fill("@marker:restored");
    await page.getByRole("button",{name:"Prévisualiser la transformation",exact:true}).click();
    await expect(preview.getByRole("heading",{name:"Effet de la transformation"})).toBeVisible();
    await expect(preview.getByRole("region",{name:"Déplacement des références",exact:true})).toBeVisible();
    await expect(preview.getByRole("heading",{name:"Layer déplacé : main",exact:true})).toBeVisible();
    await page.getByRole("dialog").getByRole("button",{name:"Appliquer la transformation",exact:true}).click();
    await expect.poll(()=>projectedText(resolve(root,"readme.txt"))).toBe("Changed contents\n");
    await open(page,"Historique");
    await page.getByRole("tab",{name:"Opérations locales",exact:true}).click();
    await expect(page.getByRole("list",{name:"Opérations locales",exact:true}).getByRole("listitem")).not.toHaveCount(0);
    await page.getByRole("tab",{name:"Captures",exact:true}).click();
    const filters=page.getByRole("form",{name:"Filtres des captures",exact:true});
    await filters.getByText("Plus de filtres",{exact:true}).click();
    await filters.getByLabel("Acteur exact",{exact:true}).fill(actor);
    await filters.getByRole("button",{name:"Afficher",exact:true}).click();
    const captures=page.getByRole("region",{name:"Captures",exact:true});
    await expect(captures.locator(".history-row").first()).toContainText(actor);
    await open(page,"Diagnostic","Diagnostic et reprise");
    await page.getByRole("button",{name:"Vérifier l’intégrité",exact:true}).click();
    await expect(page.getByText("Aucune erreur d’intégrité détectée.",{exact:true})).toBeVisible();
    await open(page,"Changements");
    await automaticCapture(page,true);
    const changed=page.getByRole("region",{name:"Fichiers modifiés"});
    await writeFile(resolve(root,"readme.txt"),"Automatically captured\n","utf8");
    await expect(changed.getByText("readme.txt",{exact:true})).toBeVisible({timeout:10000});
    await expect(page.getByText("Aucune modification de fichier.",{exact:true})).toBeVisible({timeout:15000});
    await writeFile(resolve(root,"readme.txt"),"Second automatic capture\n","utf8");
    await expect(changed.getByText("readme.txt",{exact:true})).toBeVisible({timeout:10000});
    await expect(page.getByText("Aucune modification de fichier.",{exact:true})).toBeVisible({timeout:15000});
    await open(page,"Historique");
    const automaticGroup=page.locator("details.history-group").filter({has:page.getByText("2 captures automatiques successives",{exact:true})});
    await expect(automaticGroup).toBeVisible();
    await automaticGroup.locator(":scope > summary").click();
    await expect(automaticGroup.locator("li")).toHaveCount(2);
    const automaticIds=await automaticGroup.locator("li .layrs-technical-id code").allTextContents();
    expect(new Set(automaticIds).size).toBe(2);
    await open(page,"Versions","Versions et repères");
    await expect(page.getByRole("heading",{name:"baseline",exact:true})).toBeVisible();
    await page.screenshot({path:testInfo.outputPath("native-workspace.png"),fullPage:true});
  }finally{await desktop.dispose();}
});
