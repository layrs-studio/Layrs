import test from "node:test";
import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import {createRequire} from "node:module";
import {webcrypto} from "node:crypto";
import vm from "node:vm";
const ts=createRequire(new URL("../apps/studio-desktop/package.json",import.meta.url))("typescript");
const code=ts.transpileModule(readFileSync(new URL("../apps/studio-desktop/src/partialMergeChoices.ts",import.meta.url),"utf8"),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText;
const module={exports:{}};
vm.runInNewContext(code,{module,exports:module.exports,TextEncoder,crypto:webcrypto});
const {bindPartialMergeChoices:bind,loadPartialMergeChoices:load,savePartialMergeChoices:save,clearPartialMergeChoices:clear}=module.exports;
const draft=()=>({identity:{principalId:"alice",storeId:"store:one"},repositoryViewId:"view:one",files:[{path:"docs/file",conflict:true,local:{fileId:"one"}},{path:"__proto__",conflict:true},{path:"docs/automatic",conflict:false}],local:{captureSequence:12}});
const cache=()=>{const data=new Map();return {getItem:key=>data.get(key)??null,setItem:(key,value)=>data.set(key,value),removeItem:key=>data.delete(key)};};
const plain=value=>JSON.parse(JSON.stringify(value));

test("choices survive reopening and JSON object key reordering without storing file contents",async()=>{
  const store=cache(),preview=draft(),binding=await bind("checkout","https://example.test",preview);
  save(store,binding,preview,{"docs/file":"remote",["__proto__"]:"delete"});
  const reordered={local:preview.local,files:preview.files,repositoryViewId:preview.repositoryViewId,identity:{storeId:"store:one",principalId:"alice"}};
  assert.deepEqual(plain(load(store,await bind("checkout","https://example.test",reordered),reordered)),{"docs/file":"remote",["__proto__"]:"delete"});
  assert.equal(store.getItem(binding.key).includes("fileId"),false);
});
test("choices are never reused across checkout, server, identity, View or file variants",async()=>{
  const store=cache(),preview=draft(),binding=await bind("checkout","https://example.test",preview);
  save(store,binding,preview,{"docs/file":"local"});
  for(const [root,origin] of [["other","https://example.test"],["checkout","https://other.test"]])assert.deepEqual(plain(load(store,await bind(root,origin,preview),preview)),{});
  for(const change of [p=>p.identity.principalId="bob",p=>p.repositoryViewId="view:two",p=>p.files[0].local.fileId="two",p=>p.local.captureSequence++]){
    const next=draft();change(next);assert.deepEqual(plain(load(store,await bind("checkout","https://example.test",next),next)),{});
  }
});
test("tampered, malformed and oversized cached choices fail before being offered",async()=>{
  const store=cache(),preview=draft(),binding=await bind("checkout","https://example.test",preview);
  for(const raw of ["{", "x".repeat(1024*1024+1), ...[{"docs/file":"future"},{"docs/automatic":"delete"},{"unknown":"delete"},null,[]].map(choices=>JSON.stringify({schema:1,digest:binding.digest,choices}))]){
    store.setItem(binding.key,raw);assert.throws(()=>load(store,binding,preview));
  }
});
test("quota failure is propagated so UI cannot claim a choice was saved",async()=>{
  const preview=draft(),binding=await bind("checkout","https://example.test",preview),store=cache();
  store.setItem=()=>{throw new Error("QuotaExceededError");};
  assert.throws(()=>save(store,binding,preview,{"docs/file":"local"}),/QuotaExceededError/);
});
test("applying an old window cannot erase a newer draft's saved choices",async()=>{
  const store=cache(),first=draft(),old=await bind("checkout","https://example.test",first);
  const second=draft();second.repositoryViewId="view:new";const next=await bind("checkout","https://example.test",second);
  save(store,next,second,{"docs/file":"base"});clear(store,old);
  assert.deepEqual(plain(load(store,next,second)),{"docs/file":"base"});
  clear(store,next);assert.deepEqual(plain(load(store,next,second)),{});
});
