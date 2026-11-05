import {expect,type TestInfo} from "@playwright/test";
import {spawn} from "node:child_process";
import {mkdir,readFile,stat} from "node:fs/promises";
import {join,resolve} from "node:path";
import {request as httpsRequest} from "node:https";
import {launchNativeTauriDesktop} from "./nativeTauriDesktop";

// Private fixture data stays in Node. Never attach it, log it, or send it to the page.
export type SharedFixture={origin:string;caPem:string;caFile:string;adminAccessToken:string;workspaceRoot:string;root:string;spaceId:string;layerId:string;stepId:string;firstStepId:string;releaseId:string;reviewId:string;accounts:{alice:{principalId:string;login:string;password:string};bob:{principalId:string;login:string;password:string}}};
export async function fixtureHttp(fixture:SharedFixture,method:string,path:string,body?:unknown,token=fixture.adminAccessToken):Promise<{status:number;data:any}>{
  const origin=new URL(fixture.origin);
  if(origin.protocol!=="https:"||!["127.0.0.1","localhost","[::1]"].includes(origin.hostname)||origin.username||origin.password)throw new Error("Expected disposable loopback TLS fixture");
  return new Promise((resolve,reject)=>{
    const req=httpsRequest(new URL(path,origin),{method,ca:fixture.caPem,rejectUnauthorized:true,headers:{Authorization:`Bearer ${token}`,"Content-Type":"application/json"}},response=>{
      const chunks:Buffer[]=[];let length=0;
      response.on("data",chunk=>{length+=chunk.length;if(length>4*1024*1024){response.destroy();reject(new Error("Fixture response too large"));}else chunks.push(chunk);});
      response.on("error",()=>reject(new Error("Fixture response failed")));
      response.on("end",()=>{try{resolve({status:response.statusCode??0,data:length?JSON.parse(Buffer.concat(chunks).toString("utf8")):null});}catch{reject(new Error("Fixture response is not JSON"));}});
    });
    req.setTimeout(20_000,()=>{req.destroy();reject(new Error("Fixture request timed out"));});
    req.on("error",()=>reject(new Error("Fixture TLS request failed")));
    req.end(body===undefined?undefined:JSON.stringify(body));
  });
}

export async function sharedNativeClients(testInfo:TestInfo,fixture:SharedFixture){
  const binary=resolve("target/debug",process.platform==="win32"?"layrs.exe":"layrs");
  if(!(await stat(binary)).isFile())throw new Error("Build the native CLI before this recipe");
  console.log("[shared-native] Desktop: launching");
  const desktop=await launchNativeTauriDesktop(testInfo,{visual:false});
  desktop.page.setDefaultTimeout(15_000);
  const bounded=async<T>(promise:Promise<T>,phase:string):Promise<T>=>{
    let timer:ReturnType<typeof setTimeout>|undefined;
    try{return await Promise.race([promise,new Promise<never>((_,reject)=>{timer=setTimeout(()=>reject(new Error(`Shared Desktop ${phase} timed out`)),25_000);})]);}
    finally{if(timer)clearTimeout(timer);}
  };
  const remote=<T=any>(action:Record<string,unknown>)=>bounded(desktop.invoke<T>("native_remote",{root:fixture.workspaceRoot,request:{origin:fixture.origin,caPem:fixture.caPem,action}}),String(action.action));
  let authorized=false;
  const close=async()=>{console.log("[shared-native] Desktop: cleanup");try{if(authorized)await remote({action:"logout"});}finally{await desktop.dispose();}};
  const cliRoot=join(fixture.root,"shared-cli-checkout");
  const cli=async(args:string[],remoteCommand=false):Promise<any>=>{
    const env={...process.env};for(const name of Object.keys(env))if(/^LAYRS_(?:TEST_DATABASE_URL|DATABASE_URL|BOOTSTRAP|ADMIN_ACCESS_TOKEN)/.test(name))delete env[name];
    return new Promise((resolve,reject)=>{
      const child=spawn(binary,["--json","--space",cliRoot,...(remoteCommand?["remote",...args,"--endpoint",fixture.origin,"--ca",fixture.caFile]:args)],{env,cwd:process.cwd(),windowsHide:true,stdio:["ignore","pipe","pipe"]});
      let output="",size=0;const timer=setTimeout(()=>{child.kill();reject(new Error("Shared CLI request timed out"));},30_000);
      child.stdout.on("data",chunk=>{size+=chunk.length;if(size>4*1024*1024){child.kill();reject(new Error("Shared CLI output too large"));}else output+=chunk.toString("utf8");});
      child.stderr.resume();child.on("error",()=>{clearTimeout(timer);reject(new Error("Shared CLI could not start"));});
      child.on("close",code=>{clearTimeout(timer);try{const parsed=JSON.parse(output);if(code!==0||parsed.ok!==true)throw new Error();resolve(parsed.data);}catch{reject(new Error(`Shared CLI ${args[0]} failed; private output withheld`));}});
    });
  };
  try{
    const login=await remote<{userCode:string}>({action:"startLogin"});
    const approval=await fixtureHttp(fixture,"POST","/v3/auth/devices/approve",{userCode:login.userCode});
    expect(approval.status).toBe(204);
    await expect.poll(async()=>{const reply=await remote<{status:string}>({action:"pollLogin"});authorized=reply.status==="authorized";return authorized;},{intervals:[5500],timeout:25_000}).toBe(true);
    console.log("[shared-native] Desktop: authenticated");
    await remote({action:"receive"});
    const markerName="three-client-exact-reference";
    await desktop.invoke("native_workspace",{root:fixture.workspaceRoot,request:{action:"createMarker",name:markerName,notes:"Reference verified by Desktop, CLI and Web"}});
    const publication=await remote<{receipt:{status:string}}>({action:"publish",integrate:false});
    expect(publication.receipt.status).toBe("applied");
    await mkdir(cliRoot);
    await cli(["clone",fixture.spaceId,cliRoot],true);
    const cliMarkerName="cli-roundtrip-exact-reference";
    await cli(["marker","create",cliMarkerName]);
    const cliPublication=await cli(["publish"],true);
    expect(cliPublication.receipt.status).toBe("applied");
    await remote({action:"receive"});
    await cli(["receive"],true);
    const markers=await cli(["markers"]),releases=await cli(["releases"]);
    const marker=markers.markers.find((item:any)=>item.name===markerName);
    expect(marker?.targetStepId).toBe(fixture.stepId);
    expect(releases.releases.find((item:any)=>item.releaseId===fixture.releaseId)?.targetStepId).toBe(fixture.stepId);
    expect(await readFile(join(cliRoot,"docs/readme.txt"),"utf8")).toBe(await readFile(join(fixture.workspaceRoot,"docs/readme.txt"),"utf8"));
    const source=await desktop.invoke<any>("native_workspace",{root:fixture.workspaceRoot,request:{action:"status"}});
    const second=await desktop.invoke<any>("native_workspace",{root:cliRoot,request:{action:"status"}});
    expect(source.state.worktree.worktreeId).not.toBe(second.state.worktree.worktreeId);
    expect(second.markers.find((item:any)=>item.markerId===marker.markerId)?.targetStepId).toBe(fixture.stepId);
    expect(source.markers.find((item:any)=>item.name===cliMarkerName)?.targetStepId).toBe(fixture.stepId);
    expect(second.markers.find((item:any)=>item.name===cliMarkerName)?.targetStepId).toBe(fixture.stepId);
    expect(second.releases.find((item:any)=>item.releaseId===fixture.releaseId)?.targetStepId).toBe(fixture.stepId);
    await desktop.page.goto(`${desktop.desktopUrl}/#project`);
    await desktop.page.getByLabel("Dossier",{exact:true}).fill(cliRoot);
    await desktop.page.getByRole("button",{name:"Ouvrir",exact:true}).click();
    const named=desktop.page.locator("article").filter({has:desktop.page.getByRole("heading",{name:markerName,exact:true})});
    await expect(named.getByText(fixture.stepId,{exact:true})).toBeVisible();
    const released=desktop.page.locator("article").filter({has:desktop.page.getByRole("heading",{name:"0.1.0-ui",exact:true})});
    await expect(released.getByText(fixture.stepId,{exact:true})).toBeVisible();
    const cliNamed=desktop.page.locator("article").filter({has:desktop.page.getByRole("heading",{name:cliMarkerName,exact:true})});
    await expect(cliNamed.getByText(fixture.stepId,{exact:true})).toBeVisible();
    return {desktop,remote,cli,cliRoot,markerName,cliMarkerName,markerId:marker.markerId,close};
  }catch(error){await close();throw error;}
}
