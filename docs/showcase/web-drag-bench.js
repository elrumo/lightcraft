// Slider-drag benchmark for the browser build (docs/web.md → Measuring).
//
// Serve the bundle (`cargo xtask web --serve`) and put a raw beside it, e.g.
//   ln -s ~/Pictures/some.cr3 target/web-site/raw.cr3
// open the page WITHOUT ?store=memory (the render workers read originals from browser storage), wait for the
// grid, and paste this whole file into the console (or run it through a browser tool's evaluate). It drops the
// raw on the page like a user would, opens it in Detail, then drags Exposure, Temperature and Highlights once per
// animation frame and prints one JSON line per drag:
//   imageFps     render jobs finished per second of the drag (picture updates; ~1/draft time with one worker)
//   frameMax     the slowest animation frame (ms): the main thread; should stay near the display's frame time
//   draft        what the loupe's draft budget has learned (scale, ms per megapixel)
//   settleMs     time from the release until the sharp render is on screen
(async () => {
  const RAW = new URLSearchParams(location.search).get("raw") || "/raw.cr3";
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const cmd = async (id, p) => JSON.parse((await lightcraft.command(id, JSON.stringify(p || {}))) || "null");
  for (let i = 0; i < 120; i++) {
    try { if (await cmd("library.info")) break; } catch (e) {}
    await sleep(500);
  }
  const before = (await cmd("library.info")).photos;
  const bytes = await (await fetch(RAW)).arrayBuffer();
  const dt = new DataTransfer();
  dt.items.add(new File([bytes], "bench.raw"));
  const canvas = document.getElementById("lightcraft_canvas");
  canvas.dispatchEvent(new DragEvent("dragover", { dataTransfer: dt, bubbles: true, cancelable: true }));
  canvas.dispatchEvent(new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true }));
  let id = before;
  for (let i = 0; i < 120 && id === before; i++) { await sleep(500); id = (await cmd("library.info")).photos; }
  const idle = async () => {
    const t = performance.now();
    for (;;) {
      const s = await cmd("web.stats");
      if (s.rendersInFlight === 0 && s.renderQueue === 0) return performance.now() - t;
      if (performance.now() - t > 120000) return -1;
      await sleep(250);
    }
  };
  await cmd("library.select", { ids: [id] });
  await cmd("view.detail");
  await sleep(1000);
  await idle();
  await sleep(1500);
  const drag = async (control, lo, hi, ticks) => {
    const frames = [];
    let last = performance.now(), running = true;
    const tick = (t) => { frames.push(t - last); last = t; if (running) requestAnimationFrame(tick); };
    requestAnimationFrame(tick);
    await cmd("develop.beginInteraction", { label: "bench" });
    const j0 = (await cmd("web.stats")).workers.remoteJobs, t0 = performance.now();
    for (let k = 0; k < ticks; k++) {
      await new Promise((r) => requestAnimationFrame(r));
      await cmd("develop.set", { control, value: lo + (hi - lo) * (k / (ticks - 1)) });
    }
    const ms = performance.now() - t0;
    running = false;
    const stats = await cmd("web.stats");
    await cmd("develop.endInteraction");
    const settleMs = await idle();
    const s = frames.slice().sort((a, b) => a - b);
    return {
      control,
      ticks,
      dragMs: Math.round(ms),
      imageFps: +((stats.workers.remoteJobs - j0) / (ms / 1000)).toFixed(1),
      frameMedian: +s[s.length >> 1].toFixed(1),
      frameMax: +s[s.length - 1].toFixed(1),
      draft: stats.draft,
      settleMs: Math.round(settleMs),
    };
  };
  const out = [];
  // the first drag of a photo pays for the idle workers decoding their copy: run one before the ones to read
  for (const [c, lo, hi] of [["light.exposure", -1, 1], ["light.exposure", 1, -1], ["wb.temp", 4500, 7500], ["light.highlights", 0, -80]]) {
    out.push(await drag(c, lo, hi, 240));
  }
  const report = { window: [innerWidth, innerHeight, devicePixelRatio], photo: id, drags: out };
  console.log("lightcraft-drag-bench " + JSON.stringify(report));
  return report;
})();
