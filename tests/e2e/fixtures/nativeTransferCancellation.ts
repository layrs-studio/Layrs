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
  await page.reload();
  await page.getByLabel("Dossier",{exact:true}).fill(root);
  await page.getByText("Serveur et synchronisation",{exact:true}).click();
  const remote=page.locator("details.native-remote");
  await remote.getByLabel("Serveur HTTPS",{exact:true}).fill(fixture.origin);
  await remote.getByText("Certificat privé de l’entreprise",{exact:true}).click();
  await remote.getByLabel("Autorité de certification PEM").fill(fixture.caPem);
  await remote.getByRole("button",{name:"Vérifier la session",exact:true}).click();
  await expect(remote.getByRole("button",{name:"Vérifier la session",exact:true})).toBeEnabled();
  await remote.getByLabel("Identifiant du projet distant",{exact:true}).fill(fixture.spaceId);
  await control("arm");
  try {
    await remote.getByRole("button",{name:"Cloner dans le dossier vide sélectionné",exact:true}).click();
    await expect.poll(async()=> (await control("status")).downloadReached,{timeout:15000}).toBe(true);
    await remote.getByRole("button",{name:"Annuler le transfert",exact:true}).click();
    await expect(remote.getByText("Arrêt demandé.",{exact:false})).toBeVisible();
    await control("release");
    await expect(remote.getByText("Transfert annulé avant application.",{exact:false})).toBeVisible();
    expect(await readdir(root)).toEqual([]);
    await expect(remote.getByRole("button",{name:"Annuler le transfert",exact:true})).toHaveCount(0);
    await remote.getByRole("button",{name:"Cloner dans le dossier vide sélectionné",exact:true}).click();
    await expect(page.getByRole("button",{name:"Capturer",exact:true})).toBeVisible();
    expect(await readFile(resolve(root,"docs/readme.txt"),"utf8")).toBe(await readFile(resolve(fixture.workspaceRoot,"docs/readme.txt"),"utf8"));
    await page.getByLabel("Capture automatique",{exact:true}).uncheck();
    const uploadText=`New fixture upload ${Date.now()}\n`;
    await writeFile(resolve(root,"upload.txt"),uploadText,"utf8");
    await page.getByRole("button",{name:"Capturer",exact:true}).click();
    await remote.getByText("Choisir les têtes distantes · administration",{exact:true}).click();
    await control("arm-upload");
    await remote.getByRole("button",{name:"Envoyer les captures candidates",exact:true}).click();
    await expect.poll(async()=> (await control("status")).downloadReached,{timeout:15000}).toBe(true);
    await remote.getByRole("button",{name:"Annuler le transfert",exact:true}).click();
    await expect(remote.getByText("Arrêt demandé.",{exact:false})).toBeVisible();
    await control("release");
    await expect(remote.getByText("Publication annulée avant envoi de la transaction.",{exact:false})).toBeVisible();
    expect(await readFile(resolve(root,"upload.txt"),"utf8")).toBe(uploadText);
    await control("arm-transaction");
    await remote.getByRole("button",{name:"Envoyer les captures candidates",exact:true}).click();
    await expect.poll(async()=> (await control("status")).downloadReached,{timeout:15000}).toBe(true);
    await remote.getByRole("button",{name:"Annuler le transfert",exact:true}).click();
    await expect(remote.getByText("La publication a déjà été engagée ; attendez son reçu.",{exact:true})).toBeVisible();
    await control("release");
    await expect(remote.getByText("Captures candidates admises. Les têtes distantes restent inchangées",{exact:false})).toBeVisible();
  } finally { await control("release"); }
}
