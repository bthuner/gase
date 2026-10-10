// gase in a web page: loads the WebAssembly module and runs its main loop.
//
// Read crates/web/src/lib.rs first for the big picture (memory, the
// boundary, one frame); this file is the JavaScript half of it:
//
//   boot()   storage → audio → instantiate the module with its imports →
//            gase_init → input listeners → requestAnimationFrame(frame)
//   frame()  poll gamepads → run as many emulated frames as the pacing
//            says → draw the pictures
//
// The module exports plain functions taking numbers (crates/web/src/abi.rs)
// and imports the `env` functions defined below.

import { AudioOut } from './audio.js';
import { attachFiles, attachInput, pollGamepads } from './input.js';
import { openStorage } from './storage.js';

// The frame description, a [u32; 17] in wasm memory (crates/web/src/video.rs).
const INFO = {
  FLAGS: 0, GAME_PTR: 1, GAME_W: 2, GAME_H: 3, RECT_X: 4, RECT_Y: 5, RECT_W: 6, RECT_H: 7,
  OVERLAY_PTR: 8, OVERLAY_W: 9, OVERLAY_H: 10, OVERLAY_SCALE: 11, BACKGROUND: 12,
  PACE: 13, PACE_VALUE: 14, FRAME: 15, FPS: 16,
};
const HAS_GAME = 1, HAS_OVERLAY = 2, OVERLAY_CHANGED = 4;
const PACE_TIMER = 0, PACE_AUDIO = 1, PACE_UNTHROTTLED = 2;
const REQUEST_PICK_ROM = 0, REQUEST_FULLSCREEN = 1, REQUEST_TITLE = 2;
const CAP_TOUCH = 1, CAP_KEYBOARD = 2, CAP_FULLSCREEN = 4, CAP_DROP_FILES = 8;

// Never emulate more than this many frames per display frame: after a
// stall (a background tab, a slow phone) skip ahead instead of racing.
const MAX_FRAMES_PER_TICK = 4;

// The free test ROM offered on the home screen (same file and checksum as
// scripts/fetch-test-roms.sh).
const TEST_ROM = {
  name: '240pSuite-1.23.bin',
  url: 'https://raw.githubusercontent.com/Apaczer/miyoo_tools/master/test_ROMS/MD/240pTestSuite-MD/240pSuite-1.23.bin',
  sha256: 'e6cdc5f7efe91378a77026ce40565265bb26a1f5748cca8896660d13cf9755db',
};

const $ = (id) => document.getElementById(id);
const encoder = new TextEncoder();
const decoder = new TextDecoder();

let wasm; // the module's exports
let memory; // its WebAssembly.Memory
let storage;
let audio = null;
let stopped = false;

// --- Reading and writing wasm memory -----------------------------------------
//
// `memory.buffer` is replaced by a new ArrayBuffer whenever the module's
// memory grows (the old one becomes empty, "detached"), so views are made
// on demand, never kept. Pointers arrive as signed i32: `>>> 0` makes them
// unsigned.

const bytes = (ptr, len) => new Uint8Array(memory.buffer, ptr >>> 0, len);
const text = (ptr, len) => decoder.decode(bytes(ptr, len));

/** Copy bytes into the module's inbox; returns their length. */
function send(data) {
  const ptr = wasm.gase_inbox(data.length);
  bytes(ptr, data.length).set(data);
  return data.length;
}
const sendText = (s) => send(encoder.encode(s));

// --- The imports: what Rust can call (crates/web/src/abi.rs) ------------------

const env = {
  now_ms: () => performance.now(),
  log: (ptr, len) => console.log('gase:', text(ptr, len)),
  fatal: (ptr, len) => crash(text(ptr, len)),
  file_size(keyPtr, keyLen) {
    const data = storage.get(text(keyPtr, keyLen));
    return data ? data.length : -1;
  },
  file_read(keyPtr, keyLen, dst, dstLen) {
    const data = storage.get(text(keyPtr, keyLen));
    if (data) bytes(dst, Math.min(dstLen, data.length)).set(data.subarray(0, dstLen));
  },
  file_write(keyPtr, keyLen, ptr, len) {
    const key = text(keyPtr, keyLen);
    const data = bytes(ptr, len).slice(); // a copy: the view dies with the call
    if (key.startsWith('screenshot/')) {
      download(key.slice('screenshot/'.length), data, 'image/png');
      return 1;
    }
    storage.put(key, data);
    return 0;
  },
  audio_queued: () => (audio ? audio.queued() : -1),
  audio_push(ptr, len) {
    if (audio) audio.push(new Int16Array(memory.buffer, ptr >>> 0, len));
  },
  request(kind, arg, ptr, len) {
    if (kind === REQUEST_PICK_ROM) pickRom();
    else if (kind === REQUEST_FULLSCREEN) setFullscreen(arg !== 0);
    else if (kind === REQUEST_TITLE) document.title = text(ptr, len);
  },
};

// --- Requests ---------------------------------------------------------------------

function pickRom() {
  // Browsers open a file chooser only shortly after a click or key press
  // ("transient user activation"). The request comes from the frame right
  // after the click on "Open ROM…", well within that window.
  $('file').click();
}

function setFullscreen(on) {
  const failed = () => wasm.gase_fullscreen_changed(document.fullscreenElement ? 1 : 0);
  if (on && !document.fullscreenElement) {
    document.documentElement.requestFullscreen({ navigationUI: 'hide' }).catch(failed);
  } else if (!on && document.fullscreenElement) {
    document.exitFullscreen().catch(failed);
  }
}

function download(name, data, type) {
  const url = URL.createObjectURL(new Blob([data], { type }));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

// --- ROMs ---------------------------------------------------------------------------

/** Hand a ROM to the emulator: stored first, so the recent list can reopen it. */
function openRom(name, data) {
  storage.put('rom/' + name, data);
  const nameBytes = encoder.encode(name);
  const ptr = wasm.gase_inbox(nameBytes.length + data.length);
  const inbox = bytes(ptr, nameBytes.length + data.length);
  inbox.set(nameBytes);
  inbox.set(data, nameBytes.length);
  wasm.gase_rom(nameBytes.length);
  // Ask the browser not to evict our storage under pressure (best effort).
  navigator.storage?.persist?.().catch(() => {});
}

async function openFile(file) {
  if (file.size > 16 << 20) {
    message(`${file.name} is too large to be a Mega Drive ROM.`);
    return;
  }
  openRom(file.name, new Uint8Array(await file.arrayBuffer()));
}

async function openTestRom(button) {
  const label = button.textContent;
  button.disabled = true;
  button.textContent = 'Downloading…';
  try {
    const response = await fetch(TEST_ROM.url);
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const data = new Uint8Array(await response.arrayBuffer());
    if (crypto.subtle) {
      const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', data));
      const hex = [...digest].map((b) => b.toString(16).padStart(2, '0')).join('');
      if (hex !== TEST_ROM.sha256) throw new Error('the download does not match its checksum');
    }
    openRom(TEST_ROM.name, data);
  } catch (e) {
    message(`Could not download the test ROM: ${e.message}`);
  } finally {
    button.disabled = false;
    button.textContent = label;
  }
}

// --- Drawing ------------------------------------------------------------------------

const stage = $('stage');
const gameCanvas = $('game');
const overlayCanvas = $('overlay');
const gameCtx = gameCanvas.getContext('2d');
const overlayCtx = overlayCanvas.getContext('2d');
let density = devicePixelRatio || 1;
let stageBox = stage.getBoundingClientRect();
let shown = {}; // last values written to the DOM, to touch it only on change

function place(canvas, key, x, y, w, h) {
  const style = `${x / density}px,${y / density}px,${w / density}px,${h / density}px`;
  if (shown[key] === style) return;
  shown[key] = style;
  canvas.style.left = `${x / density}px`;
  canvas.style.top = `${y / density}px`;
  canvas.style.width = `${w / density}px`;
  canvas.style.height = `${h / density}px`;
}

function setVisible(canvas, visible) {
  if (canvas.hidden === !visible) return;
  canvas.hidden = !visible;
}

/** Put the module's pictures into the two canvases. */
function draw() {
  const ptr = wasm.gase_video() >>> 0;
  const info = new Uint32Array(memory.buffer, ptr, 17);
  const signed = new Int32Array(memory.buffer, ptr, 17);
  const flags = info[INFO.FLAGS];

  const game = (flags & HAS_GAME) !== 0;
  setVisible(gameCanvas, game);
  if (game) {
    const w = info[INFO.GAME_W], h = info[INFO.GAME_H];
    if (gameCanvas.width !== w || gameCanvas.height !== h) {
      gameCanvas.width = w;
      gameCanvas.height = h;
    }
    // ImageData over wasm memory: putImageData copies straight from it.
    const pixels = new Uint8ClampedArray(memory.buffer, info[INFO.GAME_PTR], w * h * 4);
    gameCtx.putImageData(new ImageData(pixels, w, h), 0, 0);
    place(gameCanvas, 'game', signed[INFO.RECT_X], signed[INFO.RECT_Y], signed[INFO.RECT_W], signed[INFO.RECT_H]);
  }

  const overlay = (flags & HAS_OVERLAY) !== 0;
  setVisible(overlayCanvas, overlay);
  if (overlay) {
    const w = info[INFO.OVERLAY_W], h = info[INFO.OVERLAY_H], s = info[INFO.OVERLAY_SCALE];
    if (overlayCanvas.width !== w || overlayCanvas.height !== h) {
      overlayCanvas.width = w;
      overlayCanvas.height = h;
    }
    if (flags & OVERLAY_CHANGED) {
      const pixels = new Uint8ClampedArray(memory.buffer, info[INFO.OVERLAY_PTR], w * h * 4);
      overlayCtx.putImageData(new ImageData(pixels, w, h), 0, 0);
    }
    place(overlayCanvas, 'overlay', 0, 0, w * s, h * s);
  }

  const background = '#' + info[INFO.BACKGROUND].toString(16).padStart(6, '0');
  if (shown.background !== background) stage.style.background = shown.background = background;
  if (shown.game !== game) {
    shown.game = game;
    document.body.classList.toggle('playing', game);
  }
  stats.frame = info[INFO.FRAME];
}

// --- Resizing -----------------------------------------------------------------------

function watchSize() {
  const resized = (width, height) => {
    density = devicePixelRatio || 1;
    stageBox = stage.getBoundingClientRect();
    wasm.gase_resize(width, height, density);
  };
  const observer = new ResizeObserver(([entry]) => {
    // The exact size in device pixels where the browser reports it
    // (Chrome, Firefox: no rounding guesswork); otherwise CSS pixels ×
    // devicePixelRatio. The exact size is only trusted when it agrees with
    // the ratio (some emulated devices report CSS pixels there).
    const d = devicePixelRatio || 1;
    const css = entry.contentRect;
    const box = entry.devicePixelContentBoxSize?.[0];
    if (box && Math.abs(box.inlineSize - css.width * d) < 2 && Math.abs(box.blockSize - css.height * d) < 2) {
      resized(box.inlineSize, box.blockSize);
    } else {
      resized(Math.round(css.width * d), Math.round(css.height * d));
    }
  });
  try {
    observer.observe(stage, { box: 'device-pixel-content-box' });
  } catch {
    observer.observe(stage);
  }
}

/** A pointer event's position in the stage, in device pixels. */
function toStage(e) {
  return [(e.clientX - stageBox.left) * density, (e.clientY - stageBox.top) * density];
}

// --- The main loop ------------------------------------------------------------------

const stats = { fps: 0, drawFps: 0, updateMs: 0, frame: 0, audio: 'off', pace: 'timer', perTick: [] };
let pace = PACE_TIMER;
let paceValue = 60_000;
let consoleFps = 60;
let due = 0; // frames owed by the clock
let last = 0;
let counted = { at: 0, updates: 0, draws: 0, busy: 0, perTick: [0, 0, 0, 0, 0] };

function update() {
  const t = performance.now();
  pace = wasm.gase_update();
  // The pacing's details are in the frame description.
  const info = new Uint32Array(memory.buffer, wasm.gase_info() >>> 0, 17);
  paceValue = info[INFO.PACE_VALUE];
  consoleFps = info[INFO.FPS] / 1000;
  counted.busy += performance.now() - t;
  counted.updates++;
}

/** How many emulated frames this display frame needs. */
function framesDue(now) {
  const elapsed = Math.min(now - last, 250);
  last = now;
  if (pace === PACE_UNTHROTTLED) return Infinity; // bounded by time in frame()
  // By the clock: the console's own rate (59.92 or 49.70 Hz), or the
  // pacing's rate for menus. Rounding (not flooring) keeps the remainder
  // within half a frame either side, so a 60 Hz display with a little
  // jitter gets one frame per refresh rather than alternating 0 and 2.
  const fps = pace === PACE_AUDIO ? consoleFps : paceValue / 1000;
  due = Math.min(due + (elapsed * fps) / 1000, MAX_FRAMES_PER_TICK);
  if (pace === PACE_AUDIO && audio && audio.running) {
    // With sound, the audio queue steers as well (the app's Pacing::Audio:
    // keep about `target` frames queued). Only a clear deviation counts:
    // the level is an estimate without SharedArrayBuffer, and devices
    // consume audio in bursts. Small, slow drift between the sound card's
    // clock and the console's is the app's dynamic rate control's job.
    const behind = (paceValue - audio.queued()) / (audio.rate / consoleFps);
    if (behind > 2) due += 1; // about to run dry: catch up a frame
    else if (behind < -2) due = Math.max(due - 1, -1); // too much queued: let it drain
  }
  const n = Math.max(0, Math.round(due));
  due -= n;
  return Math.min(n, MAX_FRAMES_PER_TICK);
}

function frame(now) {
  if (stopped) return;
  requestAnimationFrame(frame);
  try {
    pollGamepads(gaseForInput);
    const n = framesDue(now);
    const start = performance.now();
    let ran = 0;
    while (ran < n) {
      update();
      ran++;
      // Fast-forward: as many updates as fit in ~12 ms of this frame, and
      // back to normal pacing as soon as the app says so.
      if (n === Infinity && (pace !== PACE_UNTHROTTLED || performance.now() - start > 12)) break;
    }
    counted.perTick[Math.min(ran, 4)]++;
    if (ran > 0) {
      draw();
      counted.draws++;
    }
    if (now - counted.at >= 1000) {
      const seconds = (now - counted.at) / 1000;
      stats.fps = Math.round(counted.updates / seconds);
      stats.drawFps = Math.round(counted.draws / seconds);
      stats.updateMs = counted.updates ? +(counted.busy / counted.updates).toFixed(2) : 0;
      stats.audio = audio ? (audio.running ? audio.mode : audio.ctx.state) : 'none';
      stats.queuedMs = audio?.running ? Math.round((audio.queued() * 1000) / audio.rate) : 0;
      stats.pace = ['timer', 'audio', 'fast-forward'][pace];
      stats.perTick = counted.perTick; // display frames that emulated 0, 1, 2… frames
      counted = { at: now, updates: 0, draws: 0, busy: 0, perTick: [0, 0, 0, 0, 0] };
      if (statsBox) {
        statsBox.textContent = `${stats.fps} fps · ${stats.updateMs} ms/frame · ${stats.audio}` +
          (stats.queuedMs ? ` · ${stats.queuedMs} ms queued` : '');
      }
    }
  } catch (e) {
    crash(e.message || String(e));
    throw e;
  }
}

// --- Messages and failure -----------------------------------------------------------

let statsBox = null;

function message(textContent) {
  const box = $('message');
  box.textContent = textContent;
  box.hidden = false;
  clearTimeout(message.timer);
  message.timer = setTimeout(() => (box.hidden = true), 6000);
}

function crash(reason) {
  if (stopped) return;
  stopped = true;
  console.error('gase stopped:', reason);
  const splash = $('splash');
  splash.hidden = false;
  splash.classList.add('crashed');
  $('status').textContent = `gase stopped: ${reason}. Your saves are kept; reload the page to start again.`;
}

// --- Start --------------------------------------------------------------------------

const gaseForInput = {
  get exports() {
    return wasm;
  },
  sendText,
  toStage,
  gesture() {
    audio?.resume();
  },
};

async function instantiate(url) {
  // instantiateStreaming compiles while the bytes download; it needs the
  // server to send `application/wasm`, so fall back if it does not.
  const response = fetch(url);
  try {
    return await WebAssembly.instantiateStreaming(response, { env });
  } catch {
    const buffer = await (await fetch(url)).arrayBuffer();
    return WebAssembly.instantiate(buffer, { env });
  }
}

async function boot() {
  const status = $('status');
  if (typeof WebAssembly !== 'object') {
    status.textContent = 'This browser cannot run WebAssembly.';
    return;
  }
  status.textContent = 'Opening storage…';
  storage = await openStorage();
  storage.onError = (e) => message(`The browser refused to store data: ${e?.message ?? e}`);

  try {
    audio = new AudioOut();
    await audio.start(new URL('audio-worklet.js', import.meta.url));
  } catch (e) {
    console.warn('gase: no sound', e);
    audio = null;
  }

  status.textContent = 'Loading the emulator…';
  const { instance } = await instantiate(new URL('gase_web.wasm', import.meta.url));
  wasm = instance.exports;
  memory = wasm.memory;

  const coarse = matchMedia('(pointer: coarse)').matches;
  const fine = matchMedia('(any-pointer: fine)').matches;
  let caps = CAP_DROP_FILES;
  if (coarse) caps |= CAP_TOUCH;
  if (fine || !coarse) caps |= CAP_KEYBOARD;
  if (document.fullscreenEnabled) caps |= CAP_FULLSCREEN;
  wasm.gase_init(audio ? audio.rate : 48_000, caps);

  attachInput(gaseForInput, stage);
  attachFiles($('file'), $('drop'), openFile);
  for (const type of ['click', 'touchend', 'keydown']) {
    addEventListener(type, () => audio?.resume(), { capture: true, passive: true });
  }
  document.addEventListener('fullscreenchange', () =>
    wasm.gase_fullscreen_changed(document.fullscreenElement ? 1 : 0),
  );
  // Hidden tab, locked phone, app switcher: pause into the menu and write
  // the saves now, since the page may never come back.
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'hidden') {
      wasm.gase_suspend();
      storage.flush();
    }
  });
  addEventListener('pagehide', () => {
    wasm.gase_shutdown();
    storage.flush();
  });
  $('test-rom').addEventListener('click', (e) => openTestRom(e.currentTarget));

  if (new URLSearchParams(location.search).has('stats')) {
    statsBox = $('stats');
    statsBox.hidden = false;
  }
  watchSize();
  $('splash').hidden = true;
  requestAnimationFrame((now) => {
    last = counted.at = now;
    requestAnimationFrame(frame);
  });
}

// For the browser tests and the curious (try `gase.stats` in the console).
window.gase = {
  stats,
  get exports() {
    return wasm;
  },
  openRom: (name, data) => openRom(name, data),
  /** Emulate `frames` frames flat out; returns frames per second. */
  bench(frames = 600) {
    const t = performance.now();
    wasm.gase_bench(frames);
    return Math.round((frames * 1000) / (performance.now() - t));
  },
};

boot().catch((e) => crash(e.message || String(e)));
