import { expect, type Page, type TestInfo } from "@playwright/test";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { request as httpsRequest } from "node:https";
import { resolve } from "node:path";

type Fixture = { origin:string; caPem:string; adminAccessToken:string; spaceId:string; workspaceRoot:string };
export async function verifyNativeCloneCancellation(page:Page, testInfo:TestInfo, fixture:Fixture) {
  async function control(action:string):Promise<{downloadReached:boolean}> {
    return new Promise((resolve,reject)=>{
      const request=httpsRequest(new URL(`/test-transfer-control/${action}`,fixture.origin),{method:"POST",ca:fixture.caPem,headers:{Authorization:`Bearer ${fixture.adminAccessToken}`}},response=>{
        let bytes="";
        response.setEncoding("utf8");
        response.on("data",chunk=>{bytes+=chunk;if(bytes.length>4096)response.destroy();});
        response.on("error",()=>reject(new Error("Fixture transfer control failed")));
        response.on("end",()=>{
          if(response.statusCode!==200){reject(new Error(`Fixture transfer control HTTP ${response.statusCode}`));return;}
          try{resolve(JSON.parse(bytes));}catch{reject(new Error("Invalid fixture transfer state"));}
        });
      });
      request.on("error",()=>reject(new Error("Fixture transfer control unavailable")));
      request.setTimeout(10000,()=>request.destroy());request.end();
    });
  }
  const root=testInfo.outputPath("cancelled-clone");
  await mkdir(root,{recursive:true});
  // Clone wizard: the remembered server and certificate give a checked session.
  await page.goto(`${new URL(page.url()).origin}/#/projects/new?mode=clone`);
  await expect(page.getByRole("main").getByText("Connecté",{exact:true})).toBeVisible();
  await page.getByText("Référence du projet (avancé)",{exact:true}).click();
  await page.getByLabel("Identifiant du projet distant",{exact:true}).fill(fixture.spaceId);
  await page.getByLabel("Dossier",{exact:true}).fill(root);
  const transfer=page.getByRole("region",{name:"Transfert en cours"});
  await control("arm");
  try {
    await page.getByRole("button",{name:"Récupérer le projet",exact:true}).click();
    await expect.poll(async()=> (await control("status")).downloadReached,{timeout:15000}).toBe(true);
    await transfer.getByRole("button",{name:"Annuler le transfert",exact:true}).click();
    await expect(transfer.getByText("Arrêt demandé.",{exact:false})).toBeVisible();
    await control("release");
    await expect(page.getByText("Transfert annulé avant application.",{exact:false}).first()).toBeVisible();
    expect(await readdir(root)).toEqual([]);
    await expect(page.getByRole("button",{name:"Annuler le transfert",exact:true})).toHaveCount(0);
    await page.getByRole("button",{name:"Récupérer le projet",exact:true}).click();
    await expect(page.getByRole("button",{name:"Créer une capture",exact:true})).toBeVisible();
    expect(await readFile(resolve(root,"docs/readme.txt"),"utf8")).toBe(await readFile(resolve(fixture.workspaceRoot,"docs/readme.txt"),"utf8"));
    const automatic=page.getByRole("switch",{name:"Capture automatique",exact:true});
    if((await automatic.getAttribute("aria-checked"))==="true")await automatic.click();
    const uploadText=`New fixture upload ${Date.now()}\n`;
    await writeFile(resolve(root,"upload.txt"),uploadText,"utf8");
    await expect(page.getByRole("region",{name:"Fichiers modifiés"}).getByText("upload.txt",{exact:true})).toBeVisible({timeout:15000});
    await page.getByRole("button",{name:"Créer une capture",exact:true}).press("Enter");
    await expect(page.getByText("Aucune modification de fichier.",{exact:true})).toBeVisible();
    await page.getByRole("complementary",{name:"Navigation principale"}).getByRole("link",{name:"Synchronisation",exact:true}).click();
    await page.getByText("Captures candidates et têtes publiées",{exact:true}).click();
    await control("arm-upload");
    await page.getByRole("button",{name:"Envoyer les captures candidates",exact:true}).click();
    await expect.poll(async()=> (await control("status")).downloadReached,{timeout:15000}).toBe(true);
    await transfer.getByRole("button",{name:"Annuler le transfert",exact:true}).click();
    await expect(transfer.getByText("Arrêt demandé.",{exact:false})).toBeVisible();
    await control("release");
    await expect(page.getByText("Publication annulée avant envoi de la transaction.",{exact:false}).first()).toBeVisible();
    expect(await readFile(resolve(root,"upload.txt"),"utf8")).toBe(uploadText);
    await control("arm-transaction");
    await page.getByRole("button",{name:"Envoyer les captures candidates",exact:true}).click();
    await expect.poll(async()=> (await control("status")).downloadReached,{timeout:15000}).toBe(true);
    await transfer.getByRole("button",{name:"Annuler le transfert",exact:true}).click();
    await expect(transfer.getByText("La publication a déjà été engagée ; attendez son reçu.",{exact:true})).toBeVisible();
    await control("release");
    await expect(page.getByText("Captures candidates admises. Les têtes distantes restent inchangées",{exact:false}).first()).toBeVisible();
  } finally { await control("release"); }
}
