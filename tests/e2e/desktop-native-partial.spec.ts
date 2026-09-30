import {expect,test,type Page} from "@playwright/test";
import {mkdir,readFile,writeFile,unlink,stat} from "node:fs/promises";
import {join} from "node:path";
import {launchNativeTauriDesktop} from "./fixtures/nativeTauriDesktop";
import {fixtureHttp} from "./fixtures/nativeSharedClients";
import {connectDesktop} from "./fixtures/nativeDesktopConnect";

const nav=(page:Page,label:string)=>page.getByRole("complementary",{name:"Navigation principale"}).getByRole("link",{name:label,exact:true});
const status=(page:Page,text:string)=>page.getByRole("status").filter({hasText:text}).first();
/** Publish or Receive from the persistent top-bar panel, then close it. */
async function sync(page:Page,action:string,done:string){
  await page.getByRole("button",{name:/^Synchronisation/}).click();
  const panel=page.getByRole("dialog",{name:"Synchronisation"});
  await panel.getByRole("button",{name:action,exact:true}).click();
  await expect(status(page,done)).toBeVisible();
  await panel.getByRole("button",{name:"Fermer la synchronisation"}).click();
}
async function captureAllowed(page:Page){
  await nav(page,"Changements").click();
  await page.getByRole("button",{name:"Capturer les modifications autorisées",exact:true}).click();
}

test.use({trace:"off",screenshot:"off",video:"off"});
test("Desktop migrates, reserves hidden paths and publishes real partial edits",async({},testInfo)=>{
  test.setTimeout(300_000);
  if(!process.env.LAYRS_NATIVE_UI_FIXTURE&&process.env.LAYRS_NATIVE_UI_REQUIRED==="1")throw new Error("Disposable TLS fixture required");
  test.skip(!process.env.LAYRS_NATIVE_UI_FIXTURE,"Requires disposable TLS/PostgreSQL fixture");
  const fixture=JSON.parse(await readFile(process.env.LAYRS_NATIVE_UI_FIXTURE!,"utf8"));
  const desktop=await launchNativeTauriDesktop(testInfo,{visual:false});
  try{
    const page=desktop.page;page.setDefaultTimeout(20_000);
    // The local store admits one opener per process: retry while the app itself reads it.
    const retry=async<T,>(call:()=>Promise<T>):Promise<T>=>{for(let attempt=0;;attempt++){try{return await call();}catch(error){if(attempt>=40||!String(error).includes("process-local lock"))throw error;await new Promise(resolve=>setTimeout(resolve,250));}}};
    await page.goto(`${desktop.desktopUrl}/#/projects/${encodeURIComponent(fixture.workspaceRoot)}/changes`);
    await expect(page.getByRole("heading",{name:"Changements",exact:true})).toBeVisible();
    const automatic=page.getByRole("switch",{name:"Capture automatique",exact:true});
    if((await automatic.getAttribute("aria-checked"))==="true")await automatic.click();
    await connectDesktop(page,fixture);
    const remote=<T,>(action:Record<string,unknown>)=>desktop.invoke<T>("native_remote",{root:fixture.workspaceRoot,request:{origin:fixture.origin,caPem:fixture.caPem,action}});
    // Fixture provisioning grants Bob membership after its last local receive.
    // Observe that current managed policy before creating a fresh publication.
    await remote({action:"receive"});
    await writeFile(join(fixture.workspaceRoot,"docs/allowed.txt"),"editable initial bytes\n");
    await desktop.invoke("native_workspace",{root:fixture.workspaceRoot,request:{action:"capture"}});
    const publication=await remote<{receipt:{status:string;rejections?:{code:string}[]}}>({action:"publish",integrate:false});
    expect(publication.receipt.status,JSON.stringify(publication.receipt.rejections?.map(item=>item.code))).toBe("applied");
    const permission={default:"deny",rules:[{effect:"allow",selector:{kind:"subtree",path:"docs"}},{effect:"deny",selector:{kind:"exact",path:"docs/readme.txt"}}]};
    const policy=await remote<{preview:unknown}>({action:"previewPolicyRules",rules:{schemaVersion:"layrs.space-policy-rules.v3",rules:[],pathPolicies:[{principalIds:[fixture.accounts.alice.principalId],read:permission,write:permission}]}});
    const applied=await remote<{receipt:{status:string}}>({action:"applyPolicy",preview:policy.preview});
    expect(applied.receipt.status).toBe("applied");

    // Migration: explicit, into another empty folder; the original is kept.
    const destination=join(fixture.root,"desktop-partial");await mkdir(destination);
    await nav(page,"Synchronisation").click();
    await page.getByText("Passer en accès limité",{exact:true}).click();
    await page.getByRole("button",{name:"Préparer l’accès limité…",exact:true}).click();
    const migration=page.getByRole("dialog",{name:"Passer en accès limité"});
    await migration.getByLabel("Dossier vide de destination",{exact:true}).fill(destination);
    await migration.getByLabel("Layer publié à projeter",{exact:true}).selectOption(fixture.layerId);
    await migration.getByRole("button",{name:"Créer l’accès limité et conserver ce dossier",exact:true}).click();
    await expect(page.getByText("Accès limité · sur cet ordinateur",{exact:true})).toBeVisible();
    await expect(page.getByRole("heading",{name:"Changements",exact:true})).toBeVisible();
    expect(await readFile(join(destination,"docs/allowed.txt"),"utf8")).toBe("editable initial bytes\n");
    await expect(stat(join(destination,"docs/readme.txt"))).rejects.toThrow();
    expect((await stat(join(fixture.workspaceRoot,"docs/readme.txt"))).isFile()).toBe(true);
    const originalPartial=await retry(()=>desktop.invoke<{projection:{snapshot:{baseStepId:string}}}>("native_projection",{root:destination,request:{action:"status"}}));
    // Same shell: full-project pages are absent, the target Layer and connection are remembered.
    await expect(nav(page,"Layers")).toHaveCount(0);
    await expect(page.getByRole("button",{name:/^Synchronisation/})).toContainText("Connecté");

    await nav(page,"Fichiers").click();
    const files=page.getByRole("region",{name:"Fichiers du projet",exact:true});
    await expect(files.locator("li").filter({hasText:"docs/readme.txt"})).toContainText("contenu interdit");
    for(const path of ["docs/readme.txt","DOCS/README.TXT","docs","docs/readme.txt/child"]){
      await files.getByLabel("Chemin de destination",{exact:true}).fill(path);
      await expect(files.getByRole("button",{name:"Préparer l’aperçu du changement",exact:true})).toBeDisabled();
    }
    await files.getByLabel("Chemin de destination",{exact:true}).fill("docs/created.txt");
    await files.getByRole("button",{name:"Préparer l’aperçu du changement",exact:true}).click();
    await page.getByRole("region",{name:"Aperçu de modification des chemins"}).getByRole("button",{name:"Appliquer ce changement de chemin",exact:true}).click();
    await expect(status(page,"Changement appliqué")).toBeVisible();
    expect(await readFile(join(destination,"docs/created.txt"),"utf8")).toBe("");
    await files.getByLabel("Action",{exact:true}).selectOption("rename");
    await files.getByLabel("Chemin source",{exact:true}).fill("docs/created.txt");
    await files.getByLabel("Chemin de destination",{exact:true}).fill("docs/renamed.txt");
    await files.getByRole("button",{name:"Préparer l’aperçu du changement",exact:true}).click();
    await page.getByRole("region",{name:"Aperçu de modification des chemins"}).getByRole("button",{name:"Appliquer ce changement de chemin",exact:true}).click();
    await expect(status(page,"Changement appliqué")).toBeVisible();
    await expect(stat(join(destination,"docs/created.txt"))).rejects.toThrow();
    expect(await readFile(join(destination,"docs/renamed.txt"),"utf8")).toBe("");
    await sync(page,"Publier la capture","Publication confirmée par le serveur.");

    // An external write to a reserved name blocks the capture and keeps the bytes.
    await writeFile(join(destination,"docs/readme.txt"),"external protected collision");
    await captureAllowed(page);
    await expect(page.getByRole("alert")).toContainText("projection_capture_refused");
    expect(await readFile(join(destination,"docs/readme.txt"),"utf8")).toBe("external protected collision");
    await unlink(join(destination,"docs/readme.txt"));
    await page.getByRole("button",{name:"Actualiser l’accès limité",exact:true}).first().click();
    await expect(page.getByRole("button",{name:"Capturer les modifications autorisées",exact:true})).not.toHaveAttribute("aria-disabled","true");
    await writeFile(join(destination,"docs/allowed.txt"),"editable published from Desktop\n");
    await captureAllowed(page);
    await expect(page.getByRole("region",{name:"Dernière capture locale",exact:true})).toContainText("docs/allowed.txt");
    await sync(page,"Publier la capture","Publication confirmée par le serveur.");
    await sync(page,"Recevoir","Réception vérifiée");
    const checked=await retry(()=>desktop.invoke<{namespaceOperations:number;completedPublications:number}>("native_projection",{root:destination,request:{action:"fsck"}}));
    expect(checked.namespaceOperations).toBe(2);expect(checked.completedPublications).toBe(2);
    await expect(stat(join(destination,"docs/readme.txt"))).rejects.toThrow();
    const history=await retry(()=>desktop.invoke<{captures:{sequence:number;changes:{path:string;replacement:unknown}[]}[]}>("native_projection",{root:destination,request:{action:"history",before:null,limit:50}}));
    const previous=history.captures.find(capture=>capture.changes.some(change=>change.path==="docs/allowed.txt"&&change.replacement!==null));
    expect(previous).toBeTruthy();

    // Local restore through a preview, then its inverse.
    await writeFile(join(destination,"docs/allowed.txt"),"unpublished bytes to preserve for undo\n");
    await captureAllowed(page);
    await expect(status(page,"Modifications autorisées capturées localement")).toBeVisible();
    await nav(page,"Historique").click();
    await page.getByRole("button",{name:"Actualiser (50 dernières)",exact:true}).click();
    await page.getByRole("list",{name:"Captures locales"}).getByRole("button",{name:new RegExp(`^Capture ${previous!.sequence} `)}).click();
    await page.getByRole("button",{name:"Préparer la restauration locale",exact:true}).click();
    const restore=page.getByRole("region",{name:"Aperçu de restauration locale",exact:true});
    await expect(restore).toContainText("docs/allowed.txt");
    await restore.getByRole("button",{name:"Appliquer la restauration locale",exact:true}).click();
    await expect(status(page,"Fichiers restaurés localement")).toBeVisible();
    expect(await readFile(join(destination,"docs/allowed.txt"),"utf8")).toBe("editable published from Desktop\n");
    await page.getByRole("button",{name:"Annuler la dernière restauration…",exact:true}).click();
    await restore.getByRole("button",{name:"Appliquer la restauration locale",exact:true}).click();
    await expect(status(page,"Fichiers restaurés localement")).toBeVisible();
    expect(await readFile(join(destination,"docs/allowed.txt"),"utf8")).toBe("unpublished bytes to preserve for undo\n");
    await retry(()=>desktop.invoke("native_projection",{root:destination,request:{action:"fsck"}}));
    await expect(stat(join(destination,"docs/readme.txt"))).rejects.toThrow();

    // Candidate/review integration uses the actual restricted account and server.
    await nav(page,"Équipe").click();
    await page.getByRole("button",{name:"Envoyer la capture pour revue",exact:true}).click();
    await expect(status(page,"Candidat reçu par le serveur")).toBeVisible();
    const reviews=page.getByRole("region",{name:"Revues des fichiers autorisés",exact:true});
    await expect(reviews.getByLabel("Candidat à proposer",{exact:true})).not.toHaveValue("");
    await reviews.getByLabel("Titre de la revue",{exact:true}).fill("Desktop partial review");
    await reviews.getByRole("button",{name:"Proposer ce candidat",exact:true}).click();
    await expect(reviews.getByRole("heading",{name:"Desktop partial review",exact:true})).toBeVisible();
    await reviews.getByLabel("Commentaire",{exact:true}).fill("Only authorized paths in this change.");
    await reviews.getByRole("button",{name:"Commenter",exact:true}).click();
    await expect(reviews.getByText("Only authorized paths in this change.",{exact:true})).toBeVisible();
    await expect(reviews.getByRole("button",{name:"Intégrer le candidat approuvé",exact:true})).toBeDisabled();
    const list=await fixtureHttp(fixture,"GET",`/v3/spaces/${fixture.spaceId}/scoped-reviews?limit=50`);
    const reviewId=list.data.reviews.find((review:{title:string})=>review.title==="Desktop partial review").reviewId;
    const login=await fixtureHttp(fixture,"POST","/v3/auth/login",{login:fixture.accounts.bob.login,password:fixture.accounts.bob.password});
    expect(login.status).toBe(200);
    const approval=await fixtureHttp(fixture,"POST",`/v3/spaces/${fixture.spaceId}/scoped-reviews/${reviewId}/actions`,{idempotencyKey:"desktop-scoped-bob-approval",action:"approve",body:"",anchorPath:null,replyTo:null},login.data.access_token);
    expect(approval.status).toBe(200);
    await reviews.getByRole("button",{name:"Actualiser cette revue",exact:true}).click();
    await reviews.getByRole("button",{name:"Intégrer le candidat approuvé",exact:true}).click();
    await expect(reviews.getByRole("status")).toContainText("Candidat intégré");

    // Supported rebase with a persisted choice that survives a reload.
    await writeFile(join(destination,"docs/allowed.txt"),"local conflict after review\n");
    await captureAllowed(page);
    await expect(status(page,"Modifications autorisées capturées localement")).toBeVisible();
    await page.getByRole("button",{name:"Fusionner avec la version publiée",exact:true}).click();
    const merge=page.getByRole("region",{name:"Fusion de l’accès limité",exact:true});
    await merge.getByRole("button",{name:"Préparer la fusion",exact:true}).click();
    await expect(merge.getByRole("status")).toContainText("Brouillon enregistré");
    const draft=await retry(()=>desktop.invoke<{files:{path:string;conflict:boolean}[]}>("native_projection",{root:destination,request:{action:"rebaseDraft"}}));
    expect(draft.files.find(file=>file.path==="docs/allowed.txt")?.conflict).toBe(true);
    await expect(merge.getByRole("button",{name:"Appliquer la fusion résolue",exact:true})).toBeDisabled();
    await merge.getByLabel("Version à conserver pour docs/allowed.txt",{exact:true}).selectOption("local");
    await expect(merge.getByRole("status")).toContainText("Choix enregistré sur ce poste");
    await page.reload();
    await expect(page.getByRole("heading",{name:"Fusion",exact:true})).toBeVisible();
    await merge.getByRole("button",{name:"Recharger le brouillon de fusion",exact:true}).click();
    await expect(merge.getByLabel("Version à conserver pour docs/allowed.txt",{exact:true})).toHaveValue("local");
    await merge.getByRole("button",{name:"Appliquer la fusion résolue",exact:true}).click();
    await expect(merge.getByRole("status")).toContainText("Fusion appliquée");
    expect(await readFile(join(destination,"docs/allowed.txt"),"utf8")).toBe("local conflict after review\n");

    // Directory move through a preview.
    await mkdir(join(destination,"docs/movable"));
    const partial=<T,>(action:Record<string,unknown>)=>retry(()=>desktop.invoke<T>("native_remote",{root:destination,request:{origin:fixture.origin,caPem:fixture.caPem,action}}));
    const state=await retry(()=>desktop.invoke<{projection:{generation:number}}>("native_projection",{root:destination,request:{action:"status"}}));
    const creation=await partial({action:"previewProjectionNamespace",expectedGeneration:state.projection.generation,mutation:{kind:"create",path:"docs/movable/file.txt"}});
    await partial({action:"applyProjectionNamespace",preview:creation});
    await nav(page,"Fichiers").click();
    await page.getByRole("button",{name:"Actualiser les fichiers",exact:true}).click();
    const directory=page.getByRole("region",{name:"Déplacement de dossier",exact:true});
    await directory.getByLabel("Dossier source",{exact:true}).fill("docs/movable");
    await directory.getByLabel("Dossier destination",{exact:true}).fill("docs/moved");
    await directory.getByRole("button",{name:"Préparer le déplacement du dossier",exact:true}).click();
    await directory.getByRole("button",{name:"Appliquer le déplacement du dossier",exact:true}).click();
    await expect(status(page,"Dossier déplacé")).toBeVisible();
    expect(await readFile(join(destination,"docs/moved/file.txt"),"utf8")).toBe("");
    await expect(stat(join(destination,"docs/movable"))).rejects.toThrow();

    // Restore a published version of the allowed files.
    await page.goto(`${desktop.desktopUrl}/#/projects/${encodeURIComponent(destination)}/merge`);
    await merge.getByText("Restaurer une version publiée",{exact:true}).click();
    await merge.getByLabel("Identifiant de la version publiée",{exact:true}).fill(originalPartial.projection.snapshot.baseStepId);
    await merge.getByLabel("Chemins historiques à restaurer, un par ligne",{exact:true}).fill("docs/allowed.txt");
    await merge.getByRole("button",{name:"Préparer la restauration historique",exact:true}).click();
    await expect(merge.getByRole("status")).toContainText("Aperçu historique enregistré");
    await merge.getByRole("button",{name:"Appliquer la restauration historique",exact:true}).click();
    await expect(merge.getByRole("status")).toContainText("Version historique restaurée");
    expect(await readFile(join(destination,"docs/allowed.txt"),"utf8")).toBe("editable initial bytes\n");
    expect(await readFile(join(destination,"docs/moved/file.txt"),"utf8")).toBe("");
    await retry(()=>desktop.invoke("native_projection",{root:destination,request:{action:"fsck"}}));
  }finally{await desktop.dispose();}
});
