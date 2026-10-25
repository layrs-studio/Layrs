import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
const canonical = value => value === null || typeof value !== 'object' ? JSON.stringify(value) : Array.isArray(value) ? `[${value.map(canonical).join(',')}]` : `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonical(value[key])}`).join(',')}}`;
const digest = bytes => `sha256:${createHash('sha256').update(bytes).digest('hex')}`;
const manifests = JSON.parse(readFileSync(new URL('../packages/lenses/test/builtin-manifests.golden.json', import.meta.url), 'utf8'));
const wasm = readFileSync(new URL('../crates/layrs-weave/wasm/native-lenses-v1.wasm', import.meta.url));
const source = `export function render(input) {
  if (input.kind === 'diff') return {title: input.path, blocks: [{kind:'code',text:(input.lines || []).map(line => (line.op === 'insert' ? '+ ' : line.op === 'delete' ? '- ' : '  ') + line.text).join('\\n')}]};
  return {title: input.path, blocks:[{kind: 'code', text: typeof input.text === 'string' ? input.text : 'Aperçu de ce format indisponible.'}]};
}\n`;
const packets = manifests.map(original => {
  const manifest = structuredClone(original);
  delete manifest.digest;
  manifest.version = '1.1.0';
  manifest.analyzer.capabilities = ['view', 'preview', 'diff'];
  manifest.viewer.component = 'layrs.viewer.worker.v1';
  manifest.viewer.reconcileStatuses = [];
  manifest.resolutionMethods = [];
  manifest.bundle = {format:'layrslens.v1',entrypoints:{viewer:{runtime:'esm',path:'viewer.mjs',export:'render',digest:digest(source),resources:{maxMemoryBytes:67108864,maxExecutionMs:5000,maxInputBytes:2097152,maxOutputBytes:1048576}}}};
  const files = [{path:'viewer.mjs',bytesBase64:Buffer.from(source).toString('base64')}];
  if (manifest.id === 'layrs.text' || manifest.id === 'layrs.code') {
    manifest.analyzer.capabilities.push('reconcile');
    manifest.bundle.entrypoints.reconcile = {runtime:'wasm',path:'reconcile.wasm',export:'layrs_reconcile',digest:digest(wasm),resources:{maxMemoryBytes:67108864,maxExecutionMs:5000,maxInputBytes:2097152,maxOutputBytes:1048576}};
    manifest.resolutionMethods = [{method:'existing',scope:'block'},{method:'incoming',scope:'block'},{method:'both',scope:'block'},{method:'manual',scope:'block'}];
    files.push({path:'reconcile.wasm',bytesBase64:wasm.toString('base64')});
  }
  manifest.digest = digest(canonical(manifest));
  return {manifest,files};
});
mkdirSync(new URL('../deploy/lenses/', import.meta.url), {recursive:true});
writeFileSync(new URL('../deploy/lenses/builtin-packets.json', import.meta.url), JSON.stringify(packets,null,2)+'\n');
const example = structuredClone(packets.find(packet => packet.manifest.id === 'layrs.text'));
example.files = example.files.filter(file => file.path === 'viewer.mjs');
delete example.manifest.bundle.entrypoints.reconcile;
example.manifest.analyzer.capabilities = ['view','preview','diff'];
example.manifest.resolutionMethods = [];
example.manifest.id = 'example.markdown';
example.manifest.name = 'Markdown';
example.manifest.version = '1.0.0';
example.manifest.analyzer.fileExtensions = ['md','markdown'];
example.manifest.analyzer.supportedMediaTypes = ['text/markdown'];
example.manifest.viewer.viewerId = 'example.viewer.markdown';
const markdown = `export function render(input) { return {title:input.path,blocks: input.kind === 'diff' ? [{kind:'code',text:input.lines.map(line => (line.op === 'insert' ? '+ ' : line.op === 'delete' ? '- ' : '  ') + line.text).join('\\n')}] : (input.text || '').split('\\n').slice(0,1000).map(line => ({kind:line.startsWith('#')?'heading':'text',text:line.replace(/^#+\\s*/, '')}))}; }\n`;
example.files[0].bytesBase64 = Buffer.from(markdown).toString('base64');
example.manifest.bundle.entrypoints.viewer.digest = digest(markdown);
delete example.manifest.digest;
example.manifest.digest = digest(canonical(example.manifest));
writeFileSync(new URL('../deploy/lenses/example-markdown.layrslens.json', import.meta.url), JSON.stringify(example,null,2)+'\n');
