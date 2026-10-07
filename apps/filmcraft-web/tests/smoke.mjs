// Browser smoke test of the FilmCraft web app over the Chrome DevTools Protocol.
// No npm dependencies (Node ≥ 22: global WebSocket and fetch).
//
//   cargo xtask web --serve 8765 &            # build + serve target/web/dist
//   node apps/filmcraft-web/tests/smoke.mjs --url http://127.0.0.1:8765/ --media clip.mp4 --out /tmp/fc-web
//
// Steps: load (timing), demo project, play 2 s (frames shown/dropped), import the media through
// `filmcraft.importUrl`, put it on a new sequence, export H.264 (download), screenshots of each
// step; then every mode and a tiny window. `--media` must be reachable from the page (copy it next
// to index.html). Prints a JSON report; exits non-zero on failure, including any Rust panic or
// uncaught exception in the console.
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync, readdirSync, statSync, mkdtempSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

const arg = (k, d) => {
  const i = process.argv.indexOf(`--${k}`);
  return i > 0 ? process.argv[i + 1] : d;
};
const url = arg("url", "http://127.0.0.1:8765/");
const out = arg("out", join(tmpdir(), "filmcraft-web-smoke"));
const media = arg("media", "web-test.mp4");
const chrome = arg("chrome", process.platform === "darwin" ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : "google-chrome");
const port = Number(arg("port", "9333"));
const headless = !process.argv.includes("--headed");
const ffprobe = arg("ffprobe", process.env.FILMCRAFT_FFPROBE);
const ffmpeg = arg("ffmpeg", process.env.FILMCRAFT_FFMPEG);
const expectedTone = Number(arg("tone", "0"));
if (!Number.isFinite(expectedTone) || expectedTone < 0) throw new Error("--tone must be a positive frequency in Hz");
const expectAudio = process.argv.includes("--expect-audio") || expectedTone > 0;
if (process.env.FILMCRAFT_REQUIRE_ORACLES === "1" && (!ffprobe || !ffmpeg)) throw new Error("FFmpeg and FFprobe are required for media verification");
mkdirSync(out, { recursive: true });
const downloads = join(out, "downloads");
mkdirSync(downloads, { recursive: true });

const profile = mkdtempSync(join(tmpdir(), "fc-chrome-"));
const proc = spawn(chrome, [
  ...(headless ? ["--headless=new"] : []),
  `--remote-debugging-port=${port}`,
  `--user-data-dir=${profile}`,
  "--no-first-run",
  "--no-default-browser-check",
  "--enable-unsafe-webgpu",
  "--autoplay-policy=no-user-gesture-required",
  "--window-size=1600,1000",
  "about:blank",
], { stdio: "ignore" });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let ws;
let nextId = 1;
const waiting = new Map();
const logs = [];

async function connect() {
  for (let i = 0; i < 100; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const page = list.find((t) => t.type === "page");
      if (page) {
        ws = new WebSocket(page.webSocketDebuggerUrl);
        await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
        ws.onmessage = (m) => {
          const msg = JSON.parse(m.data);
          if (msg.id && waiting.has(msg.id)) {
            waiting.get(msg.id)(msg);
            waiting.delete(msg.id);
          } else if (msg.method === "Runtime.consoleAPICalled") {
            logs.push(msg.params.args.map((a) => a.value ?? a.description ?? "").join(" "));
          } else if (msg.method === "Runtime.exceptionThrown") {
            logs.push("EXCEPTION " + JSON.stringify(msg.params.exceptionDetails.exception?.description ?? msg.params.exceptionDetails.text));
          }
        };
        return;
      }
    } catch {}
    await sleep(100);
  }
  throw new Error("Chrome did not start");
}

function send(method, params = {}) {
  const id = nextId++;
  ws.send(JSON.stringify({ id, method, params }));
  return new Promise((res, rej) => waiting.set(id, (m) => (m.error ? rej(new Error(`${method}: ${m.error.message}`)) : res(m.result))));
}

async function js(expr, timeout = 120000) {
  const r = await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true, timeout });
  if (r.exceptionDetails) throw new Error(`${expr}: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
  return r.result.value;
}

async function shot(name) {
  const r = await send("Page.captureScreenshot", { format: "png" });
  const p = join(out, `${name}.png`);
  writeFileSync(p, Buffer.from(r.data, "base64"));
  return p;
}

async function until(expr, ms = 60000, step = 100) {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    if (await js(expr)) return Date.now() - t0;
    await sleep(step);
  }
  throw new Error(`timeout waiting for ${expr}`);
}

const report = { url, steps: {} };
try {
  await connect();
  await send("Page.enable");
  await send("Runtime.enable");
  // Observe the real output graph without replacing the worklet, source or destination.
  await send("Page.addScriptToEvaluateOnNewDocument", { source: `
    window.__fcAudioAnalyzers = [];
    const NativeWorkletNode = window.AudioWorkletNode;
    if (NativeWorkletNode) window.AudioWorkletNode = class extends NativeWorkletNode {
      constructor(context, name, options) {
        super(context, name, options);
        const analyzer = context.createAnalyser();
        analyzer.fftSize = 4096;
        this.connect(analyzer);
        window.__fcAudioAnalyzers.push({analyzer, rate: context.sampleRate});
      }
    };
  ` });
  await send("Browser.setDownloadBehavior", { behavior: "allow", downloadPath: downloads }).catch(() => {});
  await send("Emulation.setDeviceMetricsOverride", { width: 1600, height: 1000, deviceScaleFactor: 1, mobile: false });
  const t0 = Date.now();
  await send("Page.navigate", { url });
  await until("!!(window.filmcraftLoad && (window.filmcraftLoad.readyMs || window.filmcraftLoad.error))", 120000);
  report.load = await js("window.filmcraftLoad");
  report.load.wallMs = Date.now() - t0;
  if (report.load.error) throw new Error(report.load.error);
  report.resources = await js("performance.getEntriesByType('resource').filter(e => /wasm|\\.js/.test(e.name)).map(e => ({name: e.name.split('/').pop(), kB: Math.round(e.transferSize / 1024), ms: Math.round(e.duration)}))");
  report.info = await js("filmcraft.info()");
  // let the UI settle (fonts, first frames)
  await sleep(1500);
  report.steps.demo = { screenshot: await shot("01-demo"), inspect: await js("filmcraft.inspect().then(i => ({window: i.window, activeSequence: i.activeSequence, fps: i.fps}))") };

  // play 2 seconds
  await js("filmcraft.request('ui.playback', {action: 'play'})");
  await sleep(2000);
  const pb = await js("filmcraft.inspect().then(i => ({playhead: i.playhead, playback: i.playback, fps: i.fps}))");
  report.steps.play = { ...pb, screenshot: await shot("02-playing") };
  await js("filmcraft.request('ui.playback', {action: 'stop'})");

  // import generated media
  const imp = await js(`filmcraft.importUrl(${JSON.stringify(media)})`);
  report.steps.import = imp;
  if (!imp.items || imp.items.length === 0) throw new Error("import failed: " + JSON.stringify(imp));
  const item = imp.items[0];
  // a sequence from the clip, then look at its first frames
  report.steps.sequence = await js(`filmcraft.execute("file.newSequence", {fromItem: ${item}, name: "Web import"})`);
  await sleep(1500);
  await js("filmcraft.request('ui.playback', {action: 'play'})");
  const audioWindows = [];
  for (let i = 0; i < 3; i++) {
    await sleep(500);
    audioWindows.push(await js(`(() => {
      const entry = window.__fcAudioAnalyzers.at(-1);
      if (!entry) return null;
      const samples = new Float32Array(entry.analyzer.fftSize);
      entry.analyzer.getFloatTimeDomainData(samples);
      const rms = Math.sqrt(samples.reduce((sum, s) => sum + s*s, 0) / samples.length);
      let re = 0, im = 0;
      for (let k = 0; k < samples.length; k++) {
        const phase = 2*Math.PI*${expectedTone || 440}*k/entry.rate;
        re += samples[k]*Math.cos(phase); im += samples[k]*Math.sin(phase);
      }
      return {rms, toneHz: ${expectedTone || 440}, toneAmplitude: 2*Math.hypot(re, im)/samples.length, rate: entry.rate};
    })()`));
  }
  report.steps.importPlay = { ...(await js("filmcraft.inspect().then(i => ({playhead: i.playhead, playback: i.playback}))")), screenshot: await shot("03-imported") };
  report.steps.importPlay.audioWindows = audioWindows;
  if (expectAudio && !audioWindows.every(w => w && w.rms > 0.01)) throw new Error("Imported playback produced silent audio");
  if (expectedTone && !audioWindows.every(w => w && w.toneAmplitude > w.rms * 0.8)) throw new Error("Imported playback did not preserve the expected tone");
  if (!(report.steps.importPlay.playhead > 0)) throw new Error("imported playback did not advance");
  await js("filmcraft.request('ui.playback', {action: 'stop'})");

  // Round-trip the edited document through the actual browser filesystem, including playhead.
  await js('filmcraft.execute("playhead.set", {seconds: 0.5})');
  const saved = await js('filmcraft.execute("sequence.inspect")');
  await js('filmcraft.execute("file.saveAs", {path: "/projects/web-edit.fcproj"})');
  await js('filmcraft.execute("file.closeProject")');
  await js('filmcraft.execute("file.open", {path: "/projects/web-edit.fcproj"})');
  const reopened = await js('filmcraft.execute("sequence.inspect")');
  report.steps.project = { before: saved, after: reopened };
  // Timeline selection is transient session state; document and saved playhead must match.
  const document = ({ selection, ...sequence }) => sequence;
  if (JSON.stringify(document(saved)) !== JSON.stringify(document(reopened))) throw new Error("browser save/reopen changed the sequence or playhead");
  report.steps.project = { path: "/projects/web-edit.fcproj", playhead: reopened.playhead, preserved: true };

  // export H.264 (stepped job; the file is offered as a download)
  const ex = await js(`filmcraft.execute("file.exportMedia", {format: "h264", path: "/exports/web-export.mp4", bitrateKbps: 4000})`);
  const te = Date.now();
  await until(`filmcraft.execute("jobs.list").then(js => js.some(j => j.id === ${ex.job} && j.finished))`, 300000, 250);
  report.steps.export = { job: ex.job, ms: Date.now() - te, jobs: await js("filmcraft.execute('jobs.list')"), files: await js("filmcraft.files()") };
  const job = report.steps.export.jobs.find(j => j.id === ex.job);
  if (job.result?.error) throw new Error("export failed: " + job.result.error);
  await sleep(1000);
  report.steps.export.downloads = readdirSync(downloads).filter((f) => !f.endsWith(".crdownload")).map((f) => ({ f, bytes: statSync(join(downloads, f)).size }));
  if (!report.steps.export.downloads.some(f => f.f.endsWith(".mp4") && f.bytes > 1000)) throw new Error("export did not download a valid-sized MP4");
  if (ffprobe && ffmpeg) {
    const run = (exe, args) => {
      const result = spawnSync(exe, args, { maxBuffer: 64 * 1024 * 1024 });
      if (result.error || result.status !== 0) throw new Error(`${exe}: ${result.error ?? result.stderr.toString()}`);
      return result.stdout;
    };
    const source = join(out, "verified-source.mp4");
    const response = await fetch(new URL(media, url));
    if (!response.ok) throw new Error(`Cannot read verification source: ${response.status}`);
    writeFileSync(source, Buffer.from(await response.arrayBuffer()));
    const movie = join(downloads, report.steps.export.downloads.find(f => f.f.endsWith(".mp4")).f);
    const probe = path => JSON.parse(run(ffprobe, ["-v", "error", "-show_streams", "-show_format", "-of", "json", path]));
    const input = probe(source), output = probe(movie);
    const video = output.streams.find(s => s.codec_type === "video");
    const inputVideo = input.streams.find(s => s.codec_type === "video");
    if (!video || video.width !== inputVideo.width || video.height !== inputVideo.height) throw new Error("Export changed the video geometry");
    if (Math.abs(Number(video.duration) - Number(inputVideo.duration)) > 0.05) throw new Error("Export truncated the video");
    const audio = output.streams.find(s => s.codec_type === "audio");
    if (input.streams.some(s => s.codec_type === "audio")) {
      if (!audio || Math.abs(Number(audio.duration) - Number(video.duration)) > 1 / audio.sample_rate + 0.000001) throw new Error("Export truncated or extended the audio track");
      const decode = path => run(ffmpeg, ["-v", "error", "-i", path, "-vn", "-ac", "1", "-ar", "48000", "-f", "f32le", "pipe:1"]);
      const before = decode(source), after = decode(movie);
      const rms = (buf, start, count) => {
        let sum = 0;
        for (let i = 0; i < count; i++) {
          const offset = (start + i) * 4;
          if (offset + 4 > buf.length) throw new Error("Decoded audio ended before the verification window");
          sum += buf.readFloatLE(offset) ** 2;
        }
        return Math.sqrt(sum / count);
      };
      const count = Math.min(9600, Math.floor(Number(video.duration) * 24000));
      const windows = [0.1, 0.4, 0.7, 1].map(f => Math.max(0, Math.floor(Number(video.duration) * 48000 * f) - count));
      report.steps.export.audioWindows = windows.map(start => {
        const original = rms(before, start, count), exported = rms(after, start, count);
        if (original > 0.001 && exported < original * 0.1) throw new Error(`Export lost audio at sample ${start}`);
        return { start, original, exported };
      });
    }
    report.steps.export.verifiedMedia = { video, audio, format: output.format };
  }
  report.steps.export.screenshot = await shot("04-exported");

  // every top-level mode and a tiny window: these used to panic (temp_dir in Export mode, a dock
  // clamp below 40 points) and freeze the canvas
  for (const mode of ["import", "export", "edit"]) {
    await js(`filmcraft.request('ui.set', {mode: ${JSON.stringify(mode)}})`);
    await sleep(500);
  }
  await send("Emulation.setDeviceMetricsOverride", { width: 120, height: 80, deviceScaleFactor: 1, mobile: false });
  await sleep(500);
  await send("Emulation.setDeviceMetricsOverride", { width: 1600, height: 1000, deviceScaleFactor: 2, mobile: false });
  await sleep(500);
  await send("Emulation.setDeviceMetricsOverride", { width: 1600, height: 1000, deviceScaleFactor: 1, mobile: false });
  report.steps.modes = { screenshot: await shot("05-modes") };
  // still alive? (a panicked app never answers)
  await Promise.race([js("filmcraft.inspect().then(() => true)"), sleep(5000).then(() => { throw new Error("app stopped answering"); })]);
  const fatal = logs.filter((l) => /panicked at|RuntimeError|EXCEPTION/.test(l));
  if (fatal.length) throw new Error("panic / uncaught exception:\n" + fatal.join("\n").slice(0, 4000));
  if (await js("window.filmcraftLoad.fatal || null")) throw new Error("fatal overlay shown");
  report.ok = true;
} catch (e) {
  report.ok = false;
  report.error = String(e && e.stack || e);
  try { report.failScreenshot = await shot("99-failure"); } catch {}
}
report.console = logs.slice(-60);
writeFileSync(join(out, "report.json"), JSON.stringify(report, null, 2));
console.log(JSON.stringify(report, null, 2));
try { ws && ws.close(); } catch {}
proc.kill();
process.exit(report.ok ? 0 : 1);
