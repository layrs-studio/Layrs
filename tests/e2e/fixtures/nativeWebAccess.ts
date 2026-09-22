import {expect,type Page} from "@playwright/test";
import {randomUUID} from "node:crypto";
import {fixtureHttp,type SharedFixture} from "./nativeSharedClients";

export async function webReleaseGate(page:Page,fixture:SharedFixture){
  console.log("[shared-native] release Gate: loading exact target");
  const base=`/v3/spaces/${fixture.spaceId}`;
  const before=await fixtureHttp(fixture,"GET",`${base}/releases/${fixture.releaseId}/publications`);
  expect(before.status).toBe(200);expect(before.data.activePublicationId).toBeNull();
  const release=page.getByRole("region",{name:"Publications des versions",exact:true});
  await release.getByRole("button",{name:"Charger les versions distantes",exact:true}).click();
  const selector=release.getByRole("combobox",{name:/^Version distante/});
  await selector.selectOption(fixture.releaseId,{timeout:15_000});
  await expect(selector).toHaveValue(fixture.releaseId);
  await expect(release.getByText(fixture.stepId,{exact:true})).toBeVisible();
  await release.getByRole("button",{name:"Publier cette version",exact:true}).click();
  await expect(release.getByRole("alert")).toContainText("release_gate_rejected");
  const after=await fixtureHttp(fixture,"GET",`${base}/releases/${fixture.releaseId}/publications`);
  expect(after.status).toBe(200);expect(after.data.activePublicationId).toBeNull();
  expect(after.data.events.length).toBe(before.data.events.length);
  console.log("[shared-native] release Gate: denial visible, journal unchanged");
}

export async function webRestrictedAccess(page:Page,fixture:SharedFixture,remote:(action:Record<string,unknown>)=>Promise<any>){
  console.log("[shared-native] restricted account: activating content policy");
  const base=`/v3/spaces/${fixture.spaceId}`;
  const activate=async(rules:unknown)=>{
    const prepared=await remote({action:"previewPolicyRules",rules});
    const applied=await remote({action:"applyPolicy",preview:prepared.preview});
    expect(applied.receipt.status).toBe("applied");
  };
  const denied={default:"allow",rules:[{effect:"deny",selector:{kind:"exact",path:"docs/readme.txt"}}]};
  await activate({schemaVersion:"layrs.space-policy-rules.v3",rules:[],pathPolicies:[{principalIds:[fixture.accounts.bob.principalId],read:denied,write:denied}]});
  await page.getByRole("button",{name:"Se déconnecter",exact:true}).click();
  await page.getByLabel("Compte",{exact:true}).fill(fixture.accounts.bob.login);
  await page.getByLabel("Mot de passe",{exact:true}).fill(fixture.accounts.bob.password);
  await page.getByRole("button",{name:"Se connecter",exact:true}).click();
  await page.getByLabel("Identifiant du projet",{exact:true}).fill(fixture.spaceId);
  await page.getByRole("button",{name:"Ouvrir le projet",exact:true}).click();
  await expect(page.getByRole("heading",{name:"Layers et captures publiées"})).toBeVisible();
  const history=page.getByRole("region",{name:"Historique publié",exact:true});
  await history.getByLabel("Capture publiée à explorer",{exact:true}).fill(fixture.stepId);
  await history.getByLabel("Chemin exact publié, facultatif",{exact:true}).fill("docs/readme.txt");
  const historyRefused=page.waitForResponse(response=>decodeURIComponent(new URL(response.url()).pathname)===`${base}/history`);
  await history.getByRole("button",{name:"Lire l’historique publié",exact:true}).click();
  expect((await historyRefused).status()).toBe(403);
  await expect(history).toHaveCount(0);
  // Restricted accounts currently cannot read published history. Reopen the
  // permitted metadata view; this recipe does not claim restricted history works.
  await page.getByRole("button",{name:"Ouvrir le projet",exact:true}).click();
  await expect(page.getByRole("heading",{name:"Layers et captures publiées"})).toBeVisible();
  await page.getByText("Fichiers distants et droits d’accès",{exact:true}).click();
  await page.getByRole("button",{name:"Charger les captures du projet",exact:true}).click();
  await page.getByRole("button",{name:"Afficher les chemins",exact:true}).click();
  const files=page.getByRole("region",{name:"Chemins du projet distant",exact:true});
  await expect(files.locator("li").filter({hasText:"docs/readme.txt"})).toContainText("Contenu interdit");
  console.log("[shared-native] restricted account: metadata visible, content locked");

  // Studio exposes no byte-download button. Check the exact same restricted
  // account against real TLS, separately from the visible metadata assertion.
  const login=await fixtureHttp(fixture,"POST","/v3/auth/login",{login:fixture.accounts.bob.login,password:fixture.accounts.bob.password});
  expect(login.status).toBe(200);
  const token=login.data.access_token;
  try{
    const bootstrap=await fixtureHttp(fixture,"GET",`${base}/bootstrap`,undefined,token);
    expect(bootstrap.status).toBe(200);
    const listing=await fixtureHttp(fixture,"POST",`${base}/projection`,{stepId:fixture.stepId,expectedPolicySetId:bootstrap.data.currentPolicySetId,expectedAuthorizationEpoch:bootstrap.data.authorizationEpoch,cursor:null,limit:100},token);
    expect(listing.status).toBe(200);
    const entry=listing.data.entries.find((item:any)=>item.path==="docs/readme.txt");
    expect(entry.contentReadable).toBe(false);
    const plan=await fixtureHttp(fixture,"POST",`${base}/fetch-plans`,{protocol:"layrs.sync.v3",type:"fetchPlanRequest",spaceId:fixture.spaceId,clientId:"client:web-acl-test",requestId:`request:${randomUUID()}`,claims:[{kind:"file",fileId:entry.target.fileId,preferredManifestIds:[],root:{kind:"step",stepId:fixture.stepId},repoPath:"docs/readme.txt"}],pageCursor:null,maxObjects:200},token);
    expect(plan.status).toBe(200);
    expect(plan.data.downloadCapabilities.length).toBe(0);
    expect(plan.data.deniedClaims.length).toBe(1);
    expect(plan.data.deniedClaims[0].code).toBe("path_denied");
  }finally{await fixtureHttp(fixture,"POST","/v3/auth/logout",{},token);}
  console.log("[shared-native] restricted account: byte capability denied");
  await activate({schemaVersion:"layrs.space-policy-rules.v1",rules:[{effect:"deny",principalIds:[fixture.accounts.bob.principalId],actions:["space.bootstrap"]}]});
  const refused=page.waitForResponse(response=>decodeURIComponent(new URL(response.url()).pathname)===`${base}/bootstrap`);
  await page.getByRole("button",{name:"Ouvrir le projet",exact:true}).click();
  expect((await refused).status()).toBe(403);
  await expect(page.getByRole("heading",{name:"Layers et captures publiées"})).toHaveCount(0);
  await expect(page.getByRole("region",{name:"Historique publié",exact:true})).toHaveCount(0);
  await expect(page.getByRole("alert").last()).toBeVisible();
  console.log("[shared-native] restricted account: metadata denied, stale views removed");
}
