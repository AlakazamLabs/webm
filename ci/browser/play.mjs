// Plays every .webm in ./files in headless Chromium and Firefox, from the
// start and after a seek, through a local HTTP server with Range support.
// Prints one line per run and exits non-zero if any run failed.
import { chromium, firefox } from 'playwright';
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

const dir = path.resolve('files');
const page = fs.readFileSync('page.html');
const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://x');
  if (url.pathname === '/') {
    res.writeHead(200, { 'content-type': 'text/html' });
    return res.end(page);
  }
  const file = path.join(dir, path.basename(url.pathname));
  if (!file.endsWith('.webm') || !fs.existsSync(file)) {
    res.writeHead(404);
    return res.end();
  }
  const size = fs.statSync(file).size;
  const m = /bytes=(\d*)-(\d*)/.exec(req.headers.range || '');
  if (m) {
    const start = m[1] ? Number(m[1]) : 0;
    const end = m[2] ? Math.min(Number(m[2]), size - 1) : size - 1;
    res.writeHead(206, {
      'content-type': 'video/webm',
      'accept-ranges': 'bytes',
      'content-range': `bytes ${start}-${end}/${size}`,
      'content-length': end - start + 1,
    });
    return fs.createReadStream(file, { start, end }).pipe(res);
  }
  res.writeHead(200, { 'content-type': 'video/webm', 'accept-ranges': 'bytes', 'content-length': size });
  fs.createReadStream(file).pipe(res);
});
await new Promise((r) => server.listen(Number(process.env.PORT || 0), '127.0.0.1', r));
const base = `http://127.0.0.1:${server.address().port}/`;

const files = fs.readdirSync(dir).filter((f) => f.endsWith('.webm')).sort();
const browsers = [
  ['chromium', () => chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] })],
  ['firefox', () => firefox.launch({ firefoxUserPrefs: { 'media.autoplay.default': 0, 'media.autoplay.blocking_policy': 0 } })],
];
let failed = 0;
let passed = 0;
for (const [name, launch] of browsers) {
  const browser = await launch();
  console.log(`== ${name} ${browser.version()}`);
  for (const f of files) {
    const hasVideo = !/^(opus|vorbis)_only/.test(f);
    const runs = [null];
    if (f.includes('.file.')) runs.push(3.2);
    for (const seek of runs) {
      const p = await browser.newPage();
      const q = `?f=${encodeURIComponent(f)}` + (seek === null ? '' : `&seek=${seek}`);
      let r;
      try {
        await p.goto(base + q);
        await p.waitForFunction(() => window.result, null, { timeout: 30000 });
        r = await p.evaluate(() => window.result);
      } catch (e) {
        r = { done: 'timeout', note: String(e).split('\n')[0] };
      }
      await p.close();
      const problems = [];
      if (r.done !== 'ended') problems.push(`ended: ${r.done} ${JSON.stringify(r.error || r.playError || r.note || '')}`);
      if (r.done === 'ended' && Math.abs(r.currentTime - 6) > 0.3) problems.push(`ended at ${r.currentTime}`);
      if (hasVideo && r.done === 'ended' && (r.videoWidth !== 320 || !(r.frames > 0))) problems.push(`video ${r.videoWidth}x${r.videoHeight}, ${r.frames} frames`);
      if (seek !== null && r.done === 'ended' && Math.abs(r.seekedTo - seek) > 0.1) problems.push(`seeked to ${r.seekedTo}`);
      const ok = problems.length === 0;
      ok ? passed++ : failed++;
      console.log(
        `${ok ? 'PASS' : 'FAIL'} ${name} ${f}${seek === null ? '' : ` seek ${seek}`}: ended at ${r.currentTime?.toFixed?.(3)} of ${r.duration}` +
          (hasVideo ? `, ${r.videoWidth}x${r.videoHeight}, ${r.frames} frames (${r.dropped} dropped)` : '') +
          (seek === null ? '' : `, seeked to ${r.seekedTo?.toFixed?.(3)}`) +
          (ok ? '' : ` | ${problems.join('; ')}`),
      );
    }
  }
  await browser.close();
}
server.close();
console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
