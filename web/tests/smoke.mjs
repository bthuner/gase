// A smoke test of the web version in a headless Chromium (Playwright).
//
//   node web/tests/smoke.mjs ROM [SCREENSHOT_DIR]
//
// Serves web/ with `python3 -m http.server`, then on a desktop-sized page:
// the home screen, a ROM opened through the file input, ~120 frames, the
// pause menu (Esc), sound started by a click, emulation speed; and on a
// phone-sized touch page: the on-screen controls. Fails on any page error.
// Needs `web/build.sh` first and the `playwright` package (not a
// dependency of the project: `npm i playwright` somewhere, then run with
// NODE_PATH pointing at its node_modules). Not run by CI yet.

import { spawn } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { chromium } = require('playwright');

const [rom, outDir = 'web-screenshots'] = process.argv.slice(2);
if (!rom) {
  console.error('usage: node web/tests/smoke.mjs ROM [SCREENSHOT_DIR]');
  process.exit(2);
}
mkdirSync(outDir, { recursive: true });
const web = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const port = 8000 + Math.floor(Math.random() * 1000);
const server = spawn('python3', ['-m', 'http.server', String(port), '-b', '127.0.0.1', '-d', web], {
  stdio: 'ignore',
});
const base = `http://127.0.0.1:${port}/`;

const failures = [];
function check(ok, what) {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`);
  if (!ok) failures.push(what);
}

async function waitForServer() {
  for (let i = 0; i < 50; i++) {
    try {
      if ((await fetch(base)).ok) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error('the HTTP server did not start');
}

/** Open the page; resolves when the emulator runs its loop. */
async function open(context, query = '?stats&nosw') {
  const page = await context.newPage();
  page.on('pageerror', (e) => failures.push(`page error: ${e.message}`));
  page.on('console', (m) => {
    if (m.type() === 'error') failures.push(`console error: ${m.text()}`);
  });
  await page.goto(base + query);
  await page.waitForFunction(() => window.gase?.exports && document.getElementById('splash').hidden);
  return page;
}

const frames = (page) => page.evaluate(() => window.gase.stats.frame);
async function runFrames(page, n) {
  const start = await frames(page);
  await page.waitForFunction((target) => window.gase.stats.frame >= target, start + n, { timeout: 30_000 });
}

async function loadRom(page) {
  await page.setInputFiles('#file', rom);
  await page.waitForFunction(() => document.body.classList.contains('playing'));
}

const shot = (page, name) => page.screenshot({ path: join(outDir, name) });

try {
  await waitForServer();
  const browser = await chromium.launch({ args: ['--autoplay-policy=user-gesture-required'] });

  // --- Desktop ---------------------------------------------------------------
  const desktop = await browser.newContext({ viewport: { width: 1280, height: 720 } });
  const page = await open(desktop);
  await page.waitForTimeout(300);
  await shot(page, '1-home.png');
  check(await page.isVisible('#test-rom'), 'home screen shows the test ROM offer');

  await loadRom(page);
  await runFrames(page, 120);
  await shot(page, '2-game.png');
  const title = await page.title();
  check(title.startsWith('gase - '), `title follows the game: "${title}"`);

  // Speed: frames per second while paced by the clock (no click yet, so
  // no sound), then flat out.
  await page.waitForTimeout(2200);
  const paced = await page.evaluate(() => ({ ...window.gase.stats }));
  console.log(`     paced: ${paced.fps} emulated fps, ${paced.drawFps} drawn fps, ${paced.updateMs} ms per update, audio: ${paced.audio}`);
  check(paced.fps >= 50 && paced.fps <= 65, 'runs at the console rate without sound');
  const bench = await page.evaluate(() => window.gase.bench(600));
  console.log(`     flat out: ${bench} fps (gase_bench, 600 frames)`);
  check(bench > 60, 'emulates faster than real time');

  // A click allows sound: pacing switches to the audio queue.
  await page.mouse.click(5, 5);
  await page.waitForTimeout(2500);
  const audio = await page.evaluate(() => ({ ...window.gase.stats }));
  console.log(`     with sound: ${audio.fps} emulated fps, ${audio.drawFps} drawn fps, audio: ${audio.audio}`);
  check(audio.audio.includes('chunks') || audio.audio.includes('ring'), 'sound is running');
  check(audio.fps >= 50 && audio.fps <= 65, 'audio pacing holds the console rate');

  await page.keyboard.press('Escape');
  await page.waitForTimeout(300);
  await shot(page, '3-pause-menu.png');
  check(await page.title().then((t) => t.includes('[menu]')), 'Esc opens the pause menu');

  // Storage survives a reload: the ROM is in the recent list, saved in
  // IndexedDB. Down selects it, Enter opens it.
  await page.reload();
  await page.waitForFunction(() => window.gase?.exports && document.getElementById('splash').hidden);
  await page.waitForTimeout(300);
  await shot(page, '6-home-recent.png');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await page.waitForFunction(() => document.body.classList.contains('playing'), null, { timeout: 5000 })
    .then(() => check(true, 'the recent list reopens the ROM after a reload'))
    .catch(() => check(false, 'the recent list reopens the ROM after a reload'));
  // "Open ROM…" asks the browser for its file chooser.
  await page.keyboard.press('Escape');
  await page.waitForTimeout(200);
  await page.mouse.click(438, 303); // "Close game" in the pause menu at 1280x720
  await page.waitForFunction(() => !document.body.classList.contains('playing'));
  await page.waitForTimeout(200);
  const chooser = page.waitForEvent('filechooser', { timeout: 3000 }).then(() => true, () => false);
  await page.mouse.click(640, 152); // "Open ROM…" on a 1280x720 home screen
  check(await chooser, '"Open ROM…" opens the file chooser');
  await desktop.close();

  // --- Phone -----------------------------------------------------------------
  const phone = await browser.newContext({
    viewport: { width: 390, height: 844 },
    deviceScaleFactor: 3,
    isMobile: true,
    hasTouch: true,
  });
  const mobile = await open(phone);
  await mobile.waitForTimeout(300);
  await shot(mobile, '4-phone-home.png');
  await loadRom(mobile);
  await runFrames(mobile, 120);
  await shot(mobile, '5-phone-touch-controls.png');
  // Tap the menu button of the on-screen controls? Its place depends on
  // the layout; a tap anywhere must at least not break anything.
  await mobile.touchscreen.tap(195, 700);
  await runFrames(mobile, 10);
  await phone.close();

  await browser.close();
} catch (e) {
  failures.push(String(e.stack || e));
} finally {
  server.kill();
}

if (failures.length) {
  console.error(`\n${failures.length} failure(s):\n` + failures.join('\n'));
  process.exit(1);
}
console.log(`\nall good; screenshots in ${outDir}`);
