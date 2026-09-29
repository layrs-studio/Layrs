import test from "node:test";
import assert from "node:assert/strict";
import { loadUiTypeScript } from "./lib/load-ui-typescript.mjs";
const {checkNativeNamespaceMutation: check,nativeProjectionReady: ready,nativePathKey: key,nativeProjectFileLabel: label} = loadUiTypeScript("native-project-files-model.ts");
const entry = (path,extra={}) => ({path,kind:"file",contentState:"materialized",access:"read_write",...extra});
const snapshot = () => ({schemaVersion:"layrs.project-files.v1",anchor:{origin:"https://example.test",principalId:"principal:alice",spaceId:"space:one",stepId:"step:one",rootTreeId:"tree:one",policySetId:"policy:one",authorizationEpoch:1,projectionId:"projection:one",generation:2},state:"ready",caseMode:"insensitive",pathPlatform:"windows",namespaceComplete:true,entries:[entry("public",{kind:"directory"}),entry("public/readme.txt"),entry("secret/key.txt",{access:"denied",contentState:"not_materialized"})],reservedPaths:[]});
const create = destination => ({kind:"create",destination});
test("ordinary permitted create, rename and move only prepare an engine request",()=>{
  assert.equal(ready(snapshot()),true);
  for(const mutation of [create("public/new.txt"),{kind:"rename",source:"public/readme.txt",destination:"public/renamed.txt"},{kind:"move",source:"public/readme.txt",destination:"other/readme.txt"}]) assert.equal(check(snapshot(),mutation).allowed,true);
});
test("reserved target blocks exact, ancestors, descendants and case aliases without prefix overreach",()=>{
  const s=snapshot();s.reservedPaths=[{path:"private/docs",reason:"local_preserved"}];
  for(const path of ["private/docs","private","private/docs/nested.txt","PRIVATE/DOCS/nested.txt"]) assert.equal(check(s,create(path)).code,"reserved_path",path);
  assert.equal(check(s,create("private/docs-two.txt")).allowed,true);
});
test("denied files implicitly reserve names even without a separate reservation row",()=>{
  for(const path of ["secret/key.txt","secret","secret/key.txt/child"]) assert.equal(check(snapshot(),create(path)).code,"reserved_path",path);
});
test("moving ancestor of inaccessible or locally retained data is forbidden",()=>{
  const s=snapshot();s.entries.push(entry("secret",{kind:"directory"}));
  assert.equal(check(s,{kind:"move",source:"secret",destination:"new-secret"}).code,"source_unavailable");
  s.reservedPaths.push({path:"public/local.txt",reason:"local_preserved"});
  assert.equal(check(s,{kind:"rename",source:"public",destination:"new-public"}).code,"source_unavailable");
});
test("Windows ADS, device names, ambiguous short names, separator and trailing aliases fail closed",()=>{
  for(const path of ["private/docs.","private/docs ","NUL.txt","x/COM1.log","x/LPT¹.txt","x/CONIN$","private/DO~1/key","public/a:stream","../a","a/./b","/root","a//b","a\\b","a\u0000b",".LAYRS/state",".git/config","foo~bar","a".repeat(256),Array(129).fill("a").join("/"),"cafe\u0301.txt"]) assert.equal(key(path,"windows","insensitive"),null,path);
});
test("Unicode normalization and case-equivalent duplicate entries invalidate namespace",()=>{
  const s=snapshot();s.entries.push(entry("caf\u00e9.txt"),entry("cafe\u0301.txt"));assert.equal(ready(s),false);
  const t=snapshot();t.entries.push(entry("PUBLIC/README.TXT"));assert.equal(ready(t),false);
});
test("posix case-sensitive names remain distinct but normalization aliases stay conservative",()=>{
  const s=snapshot();s.pathPlatform="posix";s.caseMode="sensitive";
  assert.equal(check(s,create("public/README.txt")).allowed,true);
  assert.equal(check(s,create("public/readme.txt")).code,"destination_exists");
});
test("missing, stale, unknown, incomplete or oversized provenance never enables changes",()=>{
  const variants=[null,...["stale","incomplete","future"].map(state=>({...snapshot(),state})),{...snapshot(),namespaceComplete:false},{...snapshot(),schemaVersion:"future"},{...snapshot(),anchor:null},{...snapshot(),anchor:{...snapshot().anchor,generation:NaN}},{...snapshot(),reservedPaths:[{path:"x",reason:"future"}]}];
  const missing=snapshot();delete missing.anchor.policySetId;variants.push(missing);
  for(const s of variants) assert.equal(check(s,create("new.txt")).code,"projection_unavailable");
});
test("unknown content/access states block even unrelated writes",()=>{
  for(const extra of [{access:"unknown"},{access:"future"},{contentState:"unavailable"},{contentState:"future"},{access:"denied",contentState:"materialized"}]){
    const s=snapshot();s.entries.push(entry("uncertain",extra));assert.equal(check(s,create("public/new.txt")).code,"projection_unavailable");
  }
});
test("colliding file parents, overwrites, source absence and recursive moves are refused",()=>{
  assert.equal(check(snapshot(),create("public/readme.txt/child")).code,"destination_exists");
  assert.equal(check(snapshot(),create("public")).code,"destination_exists");
  assert.equal(check(snapshot(),{kind:"move",source:"missing",destination:"new"}).code,"source_unavailable");
  assert.equal(check(snapshot(),{kind:"move",source:"public",destination:"public/nested"}).code,"source_unavailable");
});
test("labels distinguish forbidden bytes from previously retained local data",()=>{
  assert.match(label(entry("secret",{access:"denied",contentState:"not_materialized"})),/non accessible.*non matérialisé/);
  assert.match(label(entry("secret",{access:"denied",contentState:"reserved"})),/réservé.*interdit/);
  assert.match(label(entry("x",{access:"unknown"})),/actualisation/);
});
test("Windows parent directory aliases and device stems with spaces fail closed",()=>{
  assert.equal(key("CON .txt","windows","insensitive"),null);
  const s=snapshot();s.entries.push(entry("Folder/a"),entry("folder/b"));assert.equal(ready(s),false);
});
