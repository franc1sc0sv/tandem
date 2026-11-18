// `pnpm mockups:serve [--port 4173]`: serves mockups/v03 with our own support.js to view them
// and browse them in a browser (the `X.dc.html` links lead from one screen to another).
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
  console.log(`Mockups at http://127.0.0.1:${values.port}/ (for example /V3-Desk.dc.html). Ctrl+C to quit.`);
});
