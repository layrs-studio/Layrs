// Test-only HTTPS same-origin frontend. The backend certificate is validated;
// the browser pins this disposable certificate, never disables TLS globally.
import { createServer, request } from 'node:https';
import { readFile, realpath, stat } from 'node:fs/promises';
import { createHash, X509Certificate } from 'node:crypto';
import { extname, resolve, sep } from 'node:path';

export async function nativeUiProxy(fixture, directory) {
  const root = await realpath(directory);
  let origin;
  const server = createServer({ key: await readFile(fixture.tlsKeyFile), cert: fixture.caPem }, async (incoming, outgoing) => {
    try {
      const url = new URL(incoming.url, origin);
      if (incoming.headers.origin && incoming.headers.origin !== origin) { outgoing.writeHead(403).end(); return; }
      if (url.pathname.startsWith('/v3/') || url.pathname === '/auth/device') {
        const headers = { ...incoming.headers, host: new URL(fixture.origin).host };
        if (headers.origin) headers.origin = fixture.origin;
        delete headers.cookie;
        const upstream = request(new URL(url.pathname + url.search, fixture.origin), {
          method: incoming.method, headers, ca: fixture.caPem, rejectUnauthorized: true, timeout: 120_000,
        }, response => { outgoing.writeHead(response.statusCode, response.headers); response.pipe(outgoing); });
        upstream.on('error', () => { if (!outgoing.headersSent) outgoing.writeHead(502); outgoing.end(); });
        upstream.on('timeout', () => upstream.destroy());
        incoming.pipe(upstream);
        return;
      }
      if (incoming.method !== 'GET' && incoming.method !== 'HEAD') { outgoing.writeHead(405).end(); return; }
      const path = resolve(root, '.' + decodeURIComponent(url.pathname === '/' ? '/index.html' : url.pathname));
      const resolved = await realpath(path);
      if (!resolved.startsWith(root + sep) || !(await stat(resolved)).isFile()) { outgoing.writeHead(404).end(); return; }
      const type = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' }[extname(resolved)] || 'application/octet-stream';
      outgoing.writeHead(200, { 'Content-Type': type, 'Cache-Control': 'no-store' });
      outgoing.end(incoming.method === 'HEAD' ? undefined : await readFile(resolved));
    } catch { if (!outgoing.headersSent) outgoing.writeHead(404); outgoing.end(); }
  });
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  origin = `https://127.0.0.1:${server.address().port}`;
  const spki = new X509Certificate(fixture.caPem).publicKey.export({ type: 'spki', format: 'der' });
  return { origin, pin: createHash('sha256').update(spki).digest('base64'), async close() { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); } };
}
