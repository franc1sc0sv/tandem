// `pnpm mockups:serve [--port 4173]`: sirve mockups/v03 con el support.js propio para verlos
// y navegarlos en un navegador (los enlaces `X.dc.html` llevan de una pantalla a otra).
import { createServer } from 'node:http';
import { parseArgs } from 'node:util';
import { mockupAsset } from '../assets.ts';

const { values } = parseArgs({ options: { port: { type: 'string', default: '4173' } } });

const server = createServer((req, res) => {
  const url = new URL(req.url ?? '/', 'http://localhost');
  const asset = mockupAsset(url.pathname);
  if (!asset) {
    res.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
    res.end('no existe: ' + url.pathname);
    return;
  }
  res.writeHead(200, { 'content-type': asset.contentType, 'cache-control': 'no-store' });
  res.end(asset.body);
});

server.listen(Number(values.port), '127.0.0.1', () => {
  console.log(`Mockups en http://127.0.0.1:${values.port}/ (por ejemplo /V3-Puesto.dc.html). Ctrl+C para salir.`);
});
