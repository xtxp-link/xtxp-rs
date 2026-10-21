// A smoke test of xtxp-worker under `wrangler dev`: muppets seed and edit a
// document, and two streams, two WebSocket viewers and the refresh page all
// see the edit. Node 22 or later, no packages.
//
//   node tests/worker/smoke.mjs [http://127.0.0.1:8787]
import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../..');
const muppet = process.env.XTXP_MUPPET ?? `${root}/target/release/xtxp-muppet`;
const base = process.argv[2] ?? 'http://127.0.0.1:8787';
const doc = `smoke${Date.now()}`;
const page = `${base}/d/${doc}/`;
const run = (...args) => execFileSync(muppet, ['--server', base, ...args], { encoding: 'utf8', timeout: 20_000 });

function check(what, ok) {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
  if (!ok) process.exitCode = 1;
}

async function until(what, test, ms = 10_000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (await test()) return true;
    await new Promise((r) => setTimeout(r, 100));
  }
  return false;
}

// Collects everything a stream sends until `stop` is called.
function openStream(url) {
  const ctrl = new AbortController();
  const got = { text: '' };
  fetch(url, { signal: ctrl.signal })
    .then(async (res) => {
      const decoder = new TextDecoder();
      for await (const chunk of res.body) got.text += decoder.decode(chunk, { stream: true });
    })
    .catch(() => {});
  got.stop = () => ctrl.abort();
  return got;
}

function openSocket(url) {
  const got = { text: '', open: false };
  const ws = new WebSocket(url);
  ws.onopen = () => (got.open = true);
  ws.onmessage = (e) => (got.text += e.data);
  got.stop = () => ws.close();
  return got;
}

// The first run builds the Worker, which takes a few minutes.
await until('the worker', async () => (await fetch(base).catch(() => null))?.ok, 600_000);
run('init', `${root}/crates/xmesh/tests/fixtures/xtxp-logo.svg`, '--doc', doc);
check('init seeds the document', run('show', doc, '--canonical').includes('xtxp-logo'));

const redirect = await fetch(`${base}/d/${doc}?live=refresh`, { redirect: 'manual' });
check('the page without a slash redirects to it', redirect.status === 301 && redirect.headers.get('location').endsWith(`/d/${doc}/?live=refresh`));

const streams = [openStream(`${page}?live=stream&grain=node`), openStream(`${page}?live=stream&grain=region`)];
const wsUrl = `${page.replace('http', 'ws')}live?grain=node`;
const sockets = [openSocket(wsUrl), openSocket(wsUrl)];
check('streams open with the page', await until('streams', () => streams.every((s) => s.text.includes('<svg'))));
check('sockets get the whole document first', await until('sockets', () => sockets.every((s) => s.text.includes('<template for="doc"'))));

const width = String(100 + Math.floor(Math.random() * 900));
run('edit', doc, 'set-attr', '2/0', 'stroke-width', width);
const edited = `stroke-width="${width}"`;
const patched = (s) => s.text.lastIndexOf('<template for=') < s.text.lastIndexOf(edited);
check('every stream gets the patch', await until('stream patches', () => streams.every(patched)));
check('every socket gets the patch', await until('socket patches', () => sockets.every((s) => s.text.includes(edited))));

const refresh = await fetch(`${page}?live=refresh`);
const etag = refresh.headers.get('etag');
check('the refresh page has the edit', (await refresh.text()).includes(edited));
const again = await fetch(`${page}?live=refresh`, { headers: { 'if-none-match': etag } });
check('an unchanged refresh is 304', again.status === 304);
check('live.js is served beside the page', (await fetch(`${page}live.js`)).ok);
check('a muppet reads the edit back', run('show', doc, '--canonical').includes(edited));

for (const v of [...streams, ...sockets]) v.stop();
process.exit(process.exitCode ?? 0);
