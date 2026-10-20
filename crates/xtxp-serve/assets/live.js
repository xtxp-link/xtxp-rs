// XTXP live polyfill: applies the server muppet's declarative-partial-update
// patches (`<template for>`) to the page, for browsers without them, and
// over a WebSocket. Each patch replaces the nodes between its marker pair,
// parsed in the context of the pair's parent, so SVG patches give SVG.
// No framework; it goes away as browsers catch up.
(() => {
  const me = document.currentScript;
  const mode = (me && me.dataset.mode) || 'ws';
  const grain = (me && me.dataset.grain) || 'node';
  const live = (window.xtxpLive = window.xtxpLive || { applied: [], connected: false });
  live.mode = mode;
  live.grain = grain;

  // Markers are processing instructions where the browser knows them, and
  // comments (`?start name="…"`, `?end`) where it doesn't.
  const NAME = /name="([^"]*)"/;
  function marker(n) {
    if (n.nodeType === Node.PROCESSING_INSTRUCTION_NODE) {
      if (n.target === 'start') return { start: (NAME.exec(n.data) || [])[1] };
      if (n.target === 'end') return { end: true };
    } else if (n.nodeType === Node.COMMENT_NODE) {
      if (n.data.startsWith('?start')) return { start: (NAME.exec(n.data) || [])[1] };
      if (n.data.startsWith('?end')) return { end: true };
    }
    return null;
  }

  // name → { start, end }, rebuilt after each patch set.
  let pairs = new Map();
  function index() {
    pairs = new Map();
    const stack = [];
    const walk = document.createTreeWalker(
      document.body,
      NodeFilter.SHOW_COMMENT | NodeFilter.SHOW_PROCESSING_INSTRUCTION
    );
    for (let n = walk.nextNode(); n; n = walk.nextNode()) {
      const m = marker(n);
      if (!m) continue;
      if (m.start !== undefined) stack.push([m.start, n]);
      else if (stack.length) {
        const [name, start] = stack.pop();
        pairs.set(name, { start, end: n });
      }
    }
  }

  // One patch: replace the pair, markers included, with `html`.
  function applyOne(name, html) {
    const pair = pairs.get(name);
    if (!pair || !pair.start.parentNode) return false;
    const range = document.createRange();
    range.setStartBefore(pair.start);
    range.setEndAfter(pair.end);
    const fragment = range.createContextualFragment(html);
    range.deleteContents();
    range.insertNode(fragment);
    return true;
  }

  const TEMPLATE = /<template for="([^"]+)" data-seq="(\d+)" data-sent="(\d+)">([\s\S]*?)<\/template>/g;

  // A patch set: one or more templates, applied together in one view transition.
  function applySet(text) {
    const patches = [...text.matchAll(TEMPLATE)];
    if (!patches.length) return;
    const run = () => {
      index();
      for (const [, name, , , html] of patches) applyOne(name, html);
    };
    const seq = Number(patches[0][2]);
    const sent = Number(patches[0][3]);
    const done = (transition) => {
      const entry = { seq, sent, applied: Date.now(), mode, grain };
      live.applied.push(entry);
      document.dispatchEvent(new CustomEvent('xtxp:patch', { detail: { ...entry, transition } }));
    };
    if (document.startViewTransition) {
      const transition = document.startViewTransition(run);
      transition.updateCallbackDone.then(() => done(transition), () => done(null));
    } else {
      run();
      done(null);
    }
  }
  live.applySet = applySet;

  if (mode === 'stream') {
    // The page's own response delivers the templates; apply each as it lands.
    const take = (t) => {
      const html = t.outerHTML;
      t.remove();
      applySet(html);
    };
    document.querySelectorAll('template[for]').forEach(take);
    new MutationObserver((records) => {
      for (const r of records)
        for (const n of r.addedNodes)
          if (n.nodeName === 'TEMPLATE' && n.hasAttribute('for')) take(n);
    }).observe(document.body, { childList: true });
    live.connected = true;
    return;
  }

  // WebSocket: the server's first message is the whole document, so a
  // reconnect misses nothing.
  let backoff = 100;
  function connect() {
    // Relative to the page, which may be under a document's path.
    const url = new URL(`live?grain=${grain}`, location.href);
    url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(url);
    ws.onopen = () => {
      live.connected = true;
      backoff = 100;
    };
    ws.onmessage = (e) => applySet(e.data);
    ws.onclose = () => {
      live.connected = false;
      setTimeout(connect, backoff);
      backoff = Math.min(backoff * 2, 5000);
    };
  }
  connect();
})();
