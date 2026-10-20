// XTXP client metrics (`?metrics=1`): latency from the server sending a patch
// set to the page showing it, view transition time, and jank, sent to
// /metrics/client every 2 s and shown in a small overlay.
(() => {
  const params = new URLSearchParams(location.search);
  const mode = params.get('live') || 'auto';
  const grain = params.get('grain') || (mode === 'stream' ? 'region' : mode === 'ws' ? 'node' : 'page');
  let batch = [];
  let jank = 0;
  const all = [];

  function record(seq, sent, applied, vt) {
    const sample = { seq, latency_ms: applied - sent, vt_ms: vt };
    batch.push(sample);
    all.push(sample.latency_ms);
    draw();
  }

  // The polyfill (ws, or stream with polyfill=1) reports each patch set.
  document.addEventListener('xtxp:patch', (e) => {
    const { seq, sent, applied, transition } = e.detail;
    if (transition) {
      const start = performance.now();
      transition.finished.then(() => record(seq, sent, applied, performance.now() - start), () => record(seq, sent, applied, null));
    } else record(seq, sent, applied, null);
  });

  // Native stream: the server follows each patch set with a hidden beacon.
  if (mode === 'stream' && params.get('polyfill') !== '1') {
    new MutationObserver((records) => {
      for (const r of records)
        for (const n of r.addedNodes)
          if (n.nodeType === 1 && n.dataset && n.dataset.xtxpSeq)
            record(Number(n.dataset.xtxpSeq), Number(n.dataset.xtxpSent), Date.now(), null);
    }).observe(document.body || document.documentElement, { childList: true, subtree: true });
  }

  // Refresh: one sample per changed page, skipping the first of the session.
  if (mode === 'refresh' || mode === 'auto') {
    addEventListener('DOMContentLoaded', () => {
      const seq = Number(document.body.dataset.seq);
      const last = Number(sessionStorage.getItem('xtxp-seq') || 0);
      sessionStorage.setItem('xtxp-seq', String(seq));
      if (last && seq > last) record(seq, Number(document.body.dataset.sent), Date.now(), null);
    });
  }

  // Jank: frames longer than 50 ms.
  let lastFrame = performance.now();
  requestAnimationFrame(function frame(t) {
    if (t - lastFrame > 50) jank++;
    lastFrame = t;
    requestAnimationFrame(frame);
  });

  function flush() {
    if (!batch.length && !jank) return;
    navigator.sendBeacon('metrics/client', JSON.stringify({ mode, grain, samples: batch, jank }));
    batch = [];
    jank = 0;
  }
  setInterval(flush, 2000);
  addEventListener('pagehide', flush);

  let box;
  function pct(p) {
    const s = [...all].sort((a, b) => a - b);
    return s[Math.max(1, Math.ceil((p / 100) * s.length)) - 1];
  }
  function draw() {
    if (!document.body) return;
    if (!box) {
      box = document.createElement('div');
      box.style.cssText = 'position:fixed;right:8px;bottom:8px;font:12px monospace;background:#11151fcc;color:#fff;padding:6px 8px;border-radius:4px;z-index:9';
      document.body.appendChild(box);
    }
    box.textContent = `${mode}/${grain}  n=${all.length}  p50=${pct(50)}ms  p95=${pct(95)}ms`;
  }
})();
