import {expect,type Page} from "@playwright/test";
import {randomUUID} from "node:crypto";
import {fixtureHttp,type SharedFixture} from "./nativeSharedClients";

/** Private fixture data stays in Node: only names and visible text reach the page. */
export async function webSignIn(page:Page,account:{login:string;password:string}){
  await page.getByLabel("Identifiant",{exact:true}).fill(account.login);
  await page.getByLabel("Mot de passe",{exact:true}).fill(account.password);
  await page.getByRole("button",{name:"Se connecter",exact:true}).click();
  await expect(page.getByRole("heading",{name:"Vos projets"})).toBeVisible();
}

export async function webSignOut(page:Page){
  await page.getByRole("button",{name:/^Compte de /}).click();
  await page.getByRole("menuitem",{name:"Se déconnecter"}).click();
  await expect(page.getByRole("button",{name:"Se connecter",exact:true})).toBeVisible();
}

/** Opens the project by the name the catalog gives this account, never by typing its reference. */
export async function webOpenProjectByName(page:Page,fixture:SharedFixture,token=fixture.adminAccessToken){
  const catalog=await fixtureHttp(fixture,"GET","/v3/spaces?limit=100",undefined,token);
  expect(catalog.status).toBe(200);
  const entry=catalog.data.spaces.find((space:any)=>space.spaceId===fixture.spaceId);
  expect(entry,"fixture project listed in the account catalog").toBeTruthy();
  await page.locator(".project-list").getByRole("button",{name:entry.name}).click();
  await expect(page.getByRole("heading",{name:"Fichiers",exact:true})).toBeVisible();
  return entry as {name:string;access:string};
}

export async function webReleaseGate(page:Page,fixture:SharedFixture){
  console.log("[shared-native] release Gate: loading exact target");
  const base=`/v3/spaces/${fixture.spaceId}`;
  const before=await fixtureHttp(fixture,"GET",`${base}/releases/${fixture.releaseId}/publications`);
  expect(before.status).toBe(200);expect(before.data.activePublicationId).toBeNull();
  await page.getByRole("link",{name:"Versions",exact:true}).click();
  await page.getByRole("list",{name:"Versions du projet"}).getByRole("button",{name:/0\.1\.0-ui/}).click();
  const release=page.getByRole("region",{name:"Publication de la version",exact:true});
  await expect(release.getByRole("heading",{name:"0.1.0-ui",exact:true})).toBeVisible();
  await expect(release.getByText("Non publiée",{exact:true})).toBeVisible();
  await release.getByText("Détails techniques",{exact:true}).click();
  await expect(release.getByText(fixture.stepId,{exact:true})).toBeVisible();
  await release.getByRole("button",{name:"Publier cette version",exact:true}).click();
  const refusal=release.getByRole("alert");
  await expect(refusal).toContainText("Publication refusée par la protection du projet");
  await expect(refusal).toContainText("release_gate_rejected");
  const after=await fixtureHttp(fixture,"GET",`${base}/releases/${fixture.releaseId}/publications`);
  expect(after.status).toBe(200);expect(after.data.activePublicationId).toBeNull();
  expect(after.data.events.length).toBe(before.data.events.length);
  await expect(release.getByText("Jamais publiée.",{exact:true})).toBeVisible();
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
  await webSignOut(page);
  await webSignIn(page,fixture.accounts.bob);
  const login=await fixtureHttp(fixture,"POST","/v3/auth/login",{login:fixture.accounts.bob.login,password:fixture.accounts.bob.password});
  expect(login.status).toBe(200);
  const token=login.data.access_token;
  try{
    const bootstrap=await fixtureHttp(fixture,"GET",`${base}/bootstrap`,undefined,token);
    expect(bootstrap.status).toBe(200);
    const listing=await fixtureHttp(fixture,"POST",`${base}/projection`,{stepId:fixture.stepId,expectedPolicySetId:bootstrap.data.currentPolicySetId,expectedAuthorizationEpoch:bootstrap.data.authorizationEpoch,cursor:null,limit:100},token);
    expect(listing.status).toBe(200);
    const listed=listing.data.entries.find((item:any)=>item.path==="docs/readme.txt");
    expect(listed.contentReadable).toBe(false);
    const entry=await webOpenProjectByName(page,fixture,token);
    expect(entry.access).toBe("limited");
    await expect(page.getByText("Accès limité",{exact:true}).first()).toBeVisible();
    const files=page.getByRole("region",{name:"Fichiers du projet",exact:true});
    await files.getByRole("button",{name:/^docs/}).click();
    const locked=files.getByRole("list",{name:"Contenu du dossier"}).getByRole("button",{name:/readme\.txt/});
    await expect(locked).toContainText("Contenu non accessible");
    await locked.click();
    const detail=files.getByRole("complementary",{name:"Détail de la sélection"});
    await expect(detail.getByRole("status").or(detail.getByRole("alert")).filter({hasText:"Contenu non accessible"})).toBeVisible();
    // No content identifier or preview of the forbidden file reaches the page.
    await expect(detail).not.toContainText(listed.target.fileId);
    console.log("[shared-native] restricted account: metadata visible, content locked");

    // The whole-project history is not requested for a limited account; the API refusal is checked directly.
    await page.getByRole("link",{name:"Historique",exact:true}).click();
    await expect(page.getByText("Historique non accessible",{exact:true})).toBeVisible();
    await expect(page.getByRole("list",{name:"Captures publiées"})).toHaveCount(0);
    const history=await fixtureHttp(fixture,"POST",`${base}/history`,{anchor:fixture.stepId,cursor:null,limit:30,path:"docs/readme.txt",filter:{}},token);
    expect(history.status).toBe(403);

    // Studio exposes no byte download. Check the same account against real TLS.
    const plan=await fixtureHttp(fixture,"POST",`${base}/fetch-plans`,{protocol:"layrs.sync.v3",type:"fetchPlanRequest",spaceId:fixture.spaceId,clientId:"client:web-acl-test",requestId:`request:${randomUUID()}`,claims:[{kind:"file",fileId:listed.target.fileId,preferredManifestIds:[],root:{kind:"step",stepId:fixture.stepId},repoPath:"docs/readme.txt"}],pageCursor:null,maxObjects:200},token);
    expect(plan.status).toBe(200);
    expect(plan.data.downloadCapabilities.length).toBe(0);
    expect(plan.data.deniedClaims.length).toBe(1);
    expect(plan.data.deniedClaims[0].code).toBe("path_denied");
    console.log("[shared-native] restricted account: byte capability denied");

    await activate({schemaVersion:"layrs.space-policy-rules.v1",rules:[{effect:"deny",principalIds:[fixture.accounts.bob.principalId],actions:["space.bootstrap"]}]});
    const catalog=await fixtureHttp(fixture,"GET","/v3/spaces?limit=100",undefined,token);
    expect(catalog.data.spaces.some((space:any)=>space.spaceId===fixture.spaceId)).toBe(false);
    await page.getByRole("link",{name:"Tous les projets",exact:true}).click();
    await page.getByText("Ouvrir un projet par sa référence technique",{exact:true}).click();
    await page.getByLabel("Référence du projet",{exact:true}).fill(fixture.spaceId);
    const refused=page.waitForResponse(response=>decodeURIComponent(new URL(response.url()).pathname)===`${base}/bootstrap`);
    await page.getByRole("button",{name:"Ouvrir le projet",exact:true}).click();
    expect((await refused).status()).toBe(403);
    await expect(page.getByText("Ce projet n’est pas accessible",{exact:true})).toBeVisible();
    await expect(page.getByRole("region",{name:"Fichiers du projet",exact:true})).toHaveCount(0);
    await expect(page.getByRole("region",{name:"Historique publié",exact:true})).toHaveCount(0);
    console.log("[shared-native] restricted account: metadata denied, stale views removed");
  }finally{await fixtureHttp(fixture,"POST","/v3/auth/logout",{},token);}
}
