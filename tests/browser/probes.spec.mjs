// Records what the browser does, for the report; asserts nothing about it.
// Written to test-results/probes-<project>.json.
import { test } from '@playwright/test';
import { mkdirSync, writeFileSync } from 'node:fs';
import { startStack, cross } from './support.mjs';

test.describe.configure({ mode: 'serial' });

let stack;
const found = {};
test.beforeAll(async () => (stack = await startStack()));
test.afterAll(({}, info) => {
  stack?.stop();
  mkdirSync('test-results', { recursive: true });
  writeFileSync(`test-results/probes-${info.project.name}.json`, JSON.stringify(found, null, 2));
});

// Samples the pseudo-elements of running animations every frame.
function startSampling() {
  window.__pseudo = new Set();
  window.__frames = 0;
  const tick = () => {
    for (const a of document.getAnimations()) if (a.effect?.pseudoElement) window.__pseudo.add(a.effect.pseudoElement);
    if (++window.__frames < 120) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

async function transitionsDuring(page, edit) {
  await page.evaluate(startSampling);
  edit();
  await page.waitForTimeout(1200);
  return page.evaluate(() => [...window.__pseudo]);
}

let width = 20;

test('version', async ({ browser }) => {
  found.version = browser.version();
});

test('template for detection', async ({ page }) => {
  await page.goto(stack.base + '/?live=refresh');
  found.templateFor = await page.evaluate(() => {
    try {
      const d = Document.parseHTMLUnsafe('<div id="t"><?start name="t"><?end><template for="t">x</template></div>');
      return d.getElementById('t').textContent === 'x';
    } catch (e) {
      return `error: ${e}`;
    }
  });
  found.streamHTML = await page.evaluate(() => typeof Element.prototype.streamHTML);
});

test('native stream with node grain: the patched element\'s namespace', async ({ page }, info) => {
  test.skip(info.project.name !== 'chromium-158', 'needs native <template for>');
  await page.goto(stack.base + '/?live=stream&grain=node', { waitUntil: 'commit' });
  await page.waitForFunction(() => document.querySelector('main svg path.xl-c'));
  stack.edit('set-attr', '2/0', 'stroke', '#123456');
  await page.waitForTimeout(800);
  found.streamNodeGrain = await page.evaluate(() => {
    const els = [...document.querySelectorAll('main path')];
    const el = els.find((e) => e.getAttribute('stroke') === '#123456');
    return el ? { namespace: el.namespaceURI, drawn: el.getBBox ? true : false } : 'not found';
  });
});

test('does a streamed patch start a view transition by itself?', async ({ page }, info) => {
  test.skip(info.project.name !== 'chromium-158', 'needs native <template for>');
  await page.goto(stack.base + '/?live=stream', { waitUntil: 'commit' });
  await page.waitForFunction(() => document.querySelector('main svg path.xl-c'));
  found.streamStartsTransition = await transitionsDuring(page, () => stack.edit('set-attr', '2/0', 'stroke-width', String(width++)));
});

test('in ws node grain, do SVG elements get their own transition group?', async ({ page }) => {
  await page.goto(stack.base + '/?live=ws&grain=node', { waitUntil: 'commit' });
  await page.waitForFunction(() => window.xtxpLive?.connected);
  const name = await page.evaluate(() => document.querySelector('main svg path.xl-c').style.viewTransitionName);
  const pseudo = await transitionsDuring(page, () => stack.edit('set-attr', '2/0', 'stroke-width', String(width++)));
  found.wsNodeTransition = { crossName: name, pseudoElements: pseudo, crossHasGroup: pseudo.includes(`::view-transition-group(${name})`) };
});

test('cross after all of it', async ({ page }) => {
  await page.goto(stack.base + '/?live=refresh');
  found.finalCross = await page.evaluate(cross);
});
