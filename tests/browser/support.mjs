// Starts a real y-sweet, seeds the XTXP logo, and runs `xtxp-muppet serve`.
import { spawn, execFileSync } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import { join, resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../..');
const muppet = process.env.XTXP_MUPPET ?? `${root}/target/release/xtxp-muppet`;
const serve = process.env.XTXP_SERVE ?? `${root}/target/release/xtxp-serve`;
const ysweet = process.env.YSWEET_BIN ?? `${root}/.tools/bin/y-sweet`;
const LOGO = `${root}/crates/xmesh/tests/fixtures/xtxp-logo.svg`;

function freePort() {
  return new Promise((ok) => {
    const s = net.createServer();
    s.listen(0, '127.0.0.1', () => {
      const { port } = s.address();
      s.close(() => ok(port));
    });
  });
}

async function until(what, check, ms = 10_000) {
  const end = Date.now() + ms;
  for (;;) {
    try {
      if (await check()) return;
    } catch {}
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 100));
  }
}

const portOpen = (port) =>
  new Promise((ok) => {
    const c = net.connect(port, '127.0.0.1', () => (c.end(), ok(true)));
    c.on('error', () => ok(false));
  });

export async function startStack() {
  const dir = mkdtempSync(join(os.tmpdir(), 'xtxp-browser-'));
  const ysPort = await freePort();
  const ys = spawn(ysweet, ['serve', dir, '--port', String(ysPort), '--skip-gc', '--checkpoint-freq-seconds', '1'], { stdio: 'ignore' });
  await until('y-sweet', () => portOpen(ysPort));
  const server = `127.0.0.1:${ysPort}`;
  execFileSync(muppet, ['--server', server, 'init', LOGO, '--doc', 'logo'], { stdio: 'ignore' });
  const webPort = await freePort();
  const base = `http://127.0.0.1:${webPort}`;
  const log = process.env.SERVE_LOG ? (await import('node:fs')).openSync(process.env.SERVE_LOG, 'a') : 'ignore';
  const startServe = () =>
    spawn(serve, ['logo', '--server', server, '--listen', `127.0.0.1:${webPort}`], { stdio: ['ignore', 'ignore', log] });
  const live = () =>
    until('serve', async () => (await (await fetch(`${base}/?live=refresh`)).text()).includes('● live'));
  let serve = startServe();
  await live();
  return {
    base,
    edit: (...args) => execFileSync(muppet, ['--server', server, 'edit', 'logo', ...args], { stdio: 'ignore' }),
    async restartServe() {
      serve.kill();
      await new Promise((r) => serve.once('exit', r));
      serve = startServe();
      await live();
    },
    stop() {
      serve.kill();
      ys.kill();
    },
  };
}

// In-page helpers, run with page.evaluate.
export function cross() {
  const el = document.querySelector('main svg path.xl-c');
  return el && { stroke: el.getAttribute('stroke'), width: el.getAttribute('stroke-width'), ns: el.namespaceURI };
}

export function markersBalanced() {
  const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_COMMENT | NodeFilter.SHOW_PROCESSING_INSTRUCTION);
  let starts = 0;
  let ends = 0;
  for (let n = walk.nextNode(); n; n = walk.nextNode()) {
    const text = n.nodeType === 7 ? n.target : n.data;
    if (text.startsWith('start') || text.startsWith('?start')) starts++;
    if (text.startsWith('end') || text.startsWith('?end')) ends++;
  }
  return starts > 10 && starts === ends;
}

export function svgShape() {
  const svg = document.querySelector('main svg');
  return svg.outerHTML.replace(/;? ?view-transition-name: [^";]*/g, '').replace(/ style=""/g, '');
}
