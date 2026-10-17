// Export benchmark for the browser build (docs/web.md → Measuring → Export).
//
// Serve the bundle (`cargo xtask web --serve`) and put a raw beside it, e.g.
//   ln -s ~/Pictures/some.cr3 target/web-site/raw.cr3
// open the page WITHOUT ?store=memory (the export worker reads originals from browser storage), wait for the grid,
// and paste this whole file into the console (or run it through a browser tool's evaluate). Settings, on the page's
// own URL (or window.benchQuery): ?raw=/raw.cr3 (the file), ?n=10 (photos in the batch), ?size=0 (long edge, 0 = full size), ?photo=ID (a photo already in the library), ?sync (run the
// batch the way the page used to: on its own thread, the whole page frozen meanwhile; for comparing).
//
// It drops the raw on the page like a user would, exports it N times as full-size JPEGs through `app.export` (the
// Export dialog's command), keeps the page's downloads from reaching the disk (it reads each file and hashes it
// instead), and prints one JSON line:
//   frameMax / frameP99   the slowest and the 99th-percentile animation frame (ms) while the batch ran (a hidden
//                         pane gets one frame a second whatever the page does: read eventLoopGapMs then)
//   eventLoopGapMs        the longest stretch the page's thread was away from its event loop (p99 and max, ms)
//   longTasks             tasks of the main thread over 50 ms (PerformanceObserver "longtask"): count, longest, total
//   totalMs               start of the command to the last file placed
//   files                 name, size and SHA-256 of every file (same photo, same settings: all alike; and the same
//                         with ?sync, which is how "the files are the ones the page always made" is checked)
//   stats                 `web.stats` → `export` (photos on the worker / on the page, the worker's peak heap)
(async () => {
  const q = new URLSearchParams(window.benchQuery ?? location.search); // (a console can set window.benchQuery = "?n=10&sync" instead of reloading the page)
  const RAW = q.get("raw") || "/raw.cr3";
  const N = +(q.get("n") || 10);
  const SIZE = +(q.get("size") || 0);
  const SYNC = q.has("sync");
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const cmd = async (id, p) => JSON.parse((await lightcraft.command(id, JSON.stringify(p || {}))) || "null");
  for (let i = 0; i < 120; i++) {
    try { if (await cmd("library.info")) break; } catch (e) {}
    await sleep(500);
  }
  const before = (await cmd("library.info")).photos;
  let id = +q.get("photo") || before;
  if (!q.get("photo")) {
    // (a photo already in the library: ?photo=ID skips the import; the same file dropped twice is not added twice)
    const bytes = await (await fetch(RAW)).arrayBuffer();
    const dt = new DataTransfer();
    dt.items.add(new File([bytes], "bench.raw"));
    const canvas = document.getElementById("lightcraft_canvas");
    canvas.dispatchEvent(new DragEvent("dragover", { dataTransfer: dt, bubbles: true, cancelable: true }));
    canvas.dispatchEvent(new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true }));
    for (let i = 0; i < 120 && id === before; i++) { await sleep(500); id = (await cmd("library.info")).photos; }
  }
  await cmd("library.select", { ids: [id] });
  await cmd("view.detail");
  await sleep(3000);

  // the downloads: read and hash each file instead of saving it
  const hex = (b) => [...new Uint8Array(b)].map((x) => x.toString(16).padStart(2, "0")).join("");
  const got = [];
  const click = HTMLAnchorElement.prototype.click;
  HTMLAnchorElement.prototype.click = function () {
    if (!this.download) return click.call(this);
    const name = this.download;
    got.push(fetch(this.href).then((r) => r.arrayBuffer()).then(async (b) => ({ name, bytes: b.byteLength, sha256: hex(await crypto.subtle.digest("SHA-256", b)) })));
  };

  const frames = [];
  let last = performance.now(), running = true;
  const tick = (t) => { frames.push(t - last); last = t; if (running) requestAnimationFrame(tick); };
  const long = [];
  const obs = new PerformanceObserver((l) => long.push(...l.getEntries().map((e) => e.duration)));
  obs.observe({ type: "longtask" });
  requestAnimationFrame(tick);
  // How long the page's thread was away from its event loop at a stretch: a message sent to itself and back,
  // over and over. (Animation frames and timers are held back to once a second while the pane is hidden, which
  // reads as a freeze; this isn't, so it is what to trust when the pane isn't in front.)
  const lag = [];
  const chan = new MessageChannel();
  let pinging = true, ping = performance.now();
  chan.port1.onmessage = () => { const t = performance.now(); lag.push(t - ping); ping = t; if (pinging) chan.port2.postMessage(0); };
  chan.port2.postMessage(0);

  const t0 = performance.now();
  const params = { ids: Array(N).fill(id), dir: "bench", format: "jpeg", quality: 90, naming: "bench-{seq}", background: !SYNC };
  if (SIZE > 0) params.longEdge = SIZE; else params.longEdge = 0;
  const started = await cmd("app.export", params);
  // a background batch ends when web.stats says nothing runs and the last result is in; a sync one has ended already
  let stats = await cmd("web.stats");
  while (!SYNC && (stats.export.running || !stats.export.last || stats.export.last.files.length < N)) {
    if (performance.now() - t0 > 30 * 60 * 1000) break;
    await sleep(500);
    stats = await cmd("web.stats");
  }
  const totalMs = performance.now() - t0;
  await sleep(500);
  running = false;
  pinging = false;
  obs.disconnect();
  const files = await Promise.all(got);
  HTMLAnchorElement.prototype.click = click;
  const s = frames.slice(1).sort((a, b) => a - b);
  const l = lag.slice(1).sort((a, b) => a - b);
  const report = {
    mode: SYNC ? "sync (on the page)" : "background",
    window: [innerWidth, innerHeight, devicePixelRatio],
    photo: id,
    photos: N,
    longEdge: SIZE,
    started,
    totalMs: Math.round(totalMs),
    frameMedian: +s[s.length >> 1].toFixed(1),
    frameP99: +s[Math.floor(s.length * 0.99)].toFixed(1),
    frameMax: +s[s.length - 1].toFixed(1),
    eventLoopGapMs: { p99: +l[Math.floor(l.length * 0.99)].toFixed(1), max: +l[l.length - 1].toFixed(1), samples: l.length },
    longTasks: { count: long.length, maxMs: Math.round(Math.max(0, ...long)), totalMs: Math.round(long.reduce((a, b) => a + b, 0)) },
    files: files.map((f) => `${f.name} ${f.bytes} ${f.sha256.slice(0, 12)}`),
    distinctHashes: new Set(files.map((f) => f.sha256)).size,
    stats: stats.export,
  };
  delete report.stats.last;
  console.log("lightcraft-export-bench " + JSON.stringify(report));
  return report;
})();
