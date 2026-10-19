// Two Chromiums: the pinned trunk snapshot (158, with native `<template for>`)
// and Playwright's own (141, without it).
import { defineConfig } from '@playwright/test';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../..');
const snapshot =
  process.env.CHROMIUM_158 ??
  [
    `${root}/.tools/chromium-1722227/chrome-linux/chrome`,
    `${root}/.tools/chromium-1722227/chrome-mac/Chromium.app/Contents/MacOS/Chromium`,
  ].find(existsSync);

export default defineConfig({
  testDir: '.',
  timeout: 30_000,
  workers: 1,
  reporter: [['list']],
  projects: [
    { name: 'chromium-158', use: { browserName: 'chromium', launchOptions: { executablePath: snapshot } } },
    { name: 'chromium-141', use: { browserName: 'chromium' } },
  ],
});
