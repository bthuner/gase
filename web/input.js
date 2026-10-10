// Input: browser events → calls into the emulator.
//
// The translation tables live in Rust (crates/web/src/input.rs, with
// tests); this file only forwards what the browser reports, as numbers or
// short strings. Keys and pointers arrive as events. Gamepads do not: the
// Gamepad API is *polled*, so pollGamepads() runs once per frame and sends
// only what changed since the last poll.

const POINTER_KIND = { mouse: 0, touch: 1, pen: 2 };
const DOWN = 0, MOVE = 1, UP = 2, CANCEL = 3;

/**
 * Keyboard, pointer and wheel listeners.
 * `gase` provides: exports (the wasm functions), sendText(string) → length
 * (writes into the module's inbox), toStage(event) → [x, y] in physical
 * pixels, and gesture() (called on each user gesture, to unlock sound).
 */
export function attachInput(gase, stage) {
  const w = gase.exports;

  function key(e, pressed) {
    if (e.isComposing) return;
    gase.gesture();
    const len = gase.sendText(e.code);
    const used = w.gase_key(len, pressed ? 1 : 0, e.repeat ? 1 : 0);
    // Keep the browser from scrolling on Space or arrows, going back on
    // Backspace, or leaving the page on Tab, but leave its shortcuts alone.
    if (used && !e.ctrlKey && !e.metaKey && !e.altKey) e.preventDefault();
  }
  addEventListener('keydown', (e) => key(e, true));
  addEventListener('keyup', (e) => key(e, false));
  addEventListener('blur', () => w.gase_focus_lost());

  // Pointer Events unify mouse, touch and pen; every finger has its own
  // pointerId for as long as it touches the screen: that is all the
  // multi-touch the on-screen controls need.
  function pointer(e, phase) {
    const [x, y] = gase.toStage(e);
    w.gase_pointer(e.pointerId >>> 0, POINTER_KIND[e.pointerType] ?? 1, phase, x, y);
  }
  stage.addEventListener('pointerdown', (e) => {
    if (e.pointerType === 'mouse' && e.button !== 0) return;
    gase.gesture();
    // Keep receiving this pointer's moves even when it leaves the stage.
    stage.setPointerCapture(e.pointerId);
    pointer(e, DOWN);
    e.preventDefault();
  });
  stage.addEventListener('pointermove', (e) => pointer(e, MOVE));
  stage.addEventListener('pointerup', (e) => {
    if (e.pointerType === 'mouse' && e.button !== 0) return;
    gase.gesture();
    pointer(e, UP);
  });
  stage.addEventListener('pointercancel', (e) => pointer(e, CANCEL));
  // No long-press menus, no double-tap zoom over the game.
  stage.addEventListener('contextmenu', (e) => e.preventDefault());
  stage.addEventListener(
    'wheel',
    (e) => {
      w.gase_wheel(e.deltaY, e.deltaMode);
      e.preventDefault();
    },
    { passive: false },
  );
}

const pads = new Map(); // Gamepad.index → { buttons: [], axes: [] }

/** Poll every gamepad and send what changed. Call once per frame. */
export function pollGamepads(gase) {
  if (!navigator.getGamepads) return;
  const w = gase.exports;
  const seen = new Set();
  for (const pad of navigator.getGamepads()) {
    if (!pad || !pad.connected) continue;
    seen.add(pad.index);
    let state = pads.get(pad.index);
    if (!state) {
      state = { buttons: [], axes: [] };
      pads.set(pad.index, state);
      if (pad.mapping !== 'standard') {
        console.warn(`gase: "${pad.id}" has no standard mapping; buttons may be mixed up`);
      }
      w.gase_pad_connected(pad.index, gase.sendText(pad.id));
    }
    pad.buttons.forEach((button, i) => {
      if (i === 6 || i === 7) {
        // The triggers are analogue: send their value as axes 4 and 5.
        axis(i - 2, button.value);
      } else if (button.pressed !== (state.buttons[i] ?? false)) {
        state.buttons[i] = button.pressed;
        w.gase_pad_button(pad.index, i, button.pressed ? 1 : 0);
      }
    });
    for (let i = 0; i < 4 && i < pad.axes.length; i++) axis(i, pad.axes[i]);

    function axis(i, value) {
      // Sticks jitter in the last digits: only send real movements.
      const v = Math.round(value * 64) / 64;
      if (v !== (state.axes[i] ?? 0)) {
        state.axes[i] = v;
        w.gase_pad_axis(pad.index, i, v);
      }
    }
  }
  for (const index of pads.keys()) {
    if (!seen.has(index)) {
      pads.delete(index);
      w.gase_pad_disconnected(index);
    }
  }
}

/** A file chooser and drag-and-drop on the page, both calling openFile. */
export function attachFiles(fileInput, dropHint, openFile) {
  fileInput.addEventListener('change', () => {
    const file = fileInput.files[0];
    fileInput.value = '';
    if (file) openFile(file);
  });
  let depth = 0; // dragenter/dragleave fire for every child element
  addEventListener('dragenter', (e) => {
    if (![...e.dataTransfer.types].includes('Files')) return;
    depth++;
    dropHint.hidden = false;
  });
  addEventListener('dragleave', () => {
    if (--depth <= 0) {
      depth = 0;
      dropHint.hidden = true;
    }
  });
  addEventListener('dragover', (e) => e.preventDefault());
  addEventListener('drop', (e) => {
    e.preventDefault();
    depth = 0;
    dropHint.hidden = true;
    const file = e.dataTransfer.files[0];
    if (file) openFile(file);
  });
}
