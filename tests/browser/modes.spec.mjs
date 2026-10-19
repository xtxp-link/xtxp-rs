// Each delivery mode, in a browser with native `<template for>` (158) and
// one without (141): an edit from the CLI muppet reaches the page as SVG.
import { test, expect } from '@playwright/test';
import { startStack, cross, markersBalanced, svgShape } from './support.mjs';

test.describe.configure({ mode: 'serial' });

let stack;
let colour = 0;
test.beforeAll(async () => (stack = await startStack()));
test.afterAll(() => stack?.stop());

const next = () => `#00${(++colour).toString(16).padStart(2, '0')}00`;

const MODES = [
  { q: '?live=refresh', projects: ['chromium-158', 'chromium-141'] },
  { q: '?live=stream', projects: ['chromium-158'] },
  { q: '?live=stream&polyfill=1', projects: ['chromium-141'] },
  { q: '?live=ws&grain=node', projects: ['chromium-158', 'chromium-141'] },
  { q: '?live=ws&grain=region', projects: ['chromium-158', 'chromium-141'] },
];

for (const { q, projects } of MODES) {
  test(`an edit reaches ${q} as SVG`, async ({ page }, info) => {
    test.skip(!projects.includes(info.project.name), `not run in ${info.project.name}`);
    await page.goto(stack.base + '/' + q, { waitUntil: 'commit' });
    await expect.poll(() => page.evaluate(cross)).not.toBeNull();
    const want = next();
    stack.edit('set-attr', '2/0', 'stroke', want);
    await expect.poll(() => page.evaluate(cross), { timeout: 3000 }).toEqual({
      stroke: want,
      width: expect.any(String),
      ns: 'http://www.w3.org/2000/svg',
    });
    expect(await page.evaluate(markersBalanced)).toBe(true);
  });
}

for (const q of ['?live=stream', '?live=ws&grain=node']) {
  test(`patches in ${q} parse back to the rendered tree`, async ({ page, browser }, info) => {
    test.skip(q.includes('stream') && info.project.name !== 'chromium-158', 'native stream needs 158');
    await page.goto(stack.base + '/' + q, { waitUntil: 'commit' });
    await expect.poll(() => page.evaluate(cross)).not.toBeNull();
    for (const w of ['11', '12', '13']) stack.edit('set-attr', '2/0', 'stroke-width', w);
    await expect.poll(() => page.evaluate(cross).then((c) => c.width), { timeout: 3000 }).toBe('13');
    const fresh = await browser.newPage();
    await fresh.goto(stack.base + '/?live=refresh');
    expect(await page.evaluate(svgShape)).toBe(await fresh.evaluate(svgShape));
    await fresh.close();
  });
}

test('no parameters picks stream in 158 and the WebSocket in 141', async ({ page }, info) => {
  await page.goto(stack.base + '/', { waitUntil: 'commit' });
  if (info.project.name === 'chromium-158') {
    await expect(page).toHaveURL(/\?live=stream$/);
  } else {
    await expect.poll(() => page.evaluate(() => window.xtxpLive?.connected === true)).toBe(true);
  }
});

test('JavaScript off gets the refresh loop', async ({ browser }) => {
  const context = await browser.newContext({ javaScriptEnabled: false });
  const page = await context.newPage();
  await page.goto(stack.base + '/');
  const want = next();
  stack.edit('set-attr', '2/0', 'stroke', want);
  await expect(page.locator('main svg path.xl-c')).toHaveAttribute('stroke', want, { timeout: 4000 });
  await context.close();
});

test('live.js reconnects after the server drops it', async ({ page }) => {
  await page.goto(stack.base + '/?live=ws', { waitUntil: 'commit' });
  await expect.poll(() => page.evaluate(() => window.xtxpLive?.connected === true)).toBe(true);
  await stack.restartServe();
  await expect.poll(() => page.evaluate(() => window.xtxpLive?.connected === true), { timeout: 10_000 }).toBe(true);
  const want = next();
  stack.edit('set-attr', '2/0', 'stroke', want);
  await expect.poll(() => page.evaluate(cross).then((c) => c.stroke), { timeout: 3000 }).toBe(want);
});
