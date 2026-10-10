//! The boundary with JavaScript: every function the page can call, and
//! every function the module calls in the page.
//!
//! This is the only module allowed to use what the `unsafe_code` lint
//! flags, and it contains no `unsafe { }` block:
//!
//! * `#[unsafe(no_mangle)]` keeps an export's name as written, so the
//!   page can call `instance.exports.gase_update()`. The attribute is
//!   "unsafe" because two functions exported under the same symbol name
//!   would clash at link time and could make the program call the wrong
//!   one; every name here is unique and prefixed `gase_`.
//! * The `unsafe extern "C"` block declares the imports. Calling a foreign
//!   function is unsafe in general because Rust cannot check what it does;
//!   writing `safe fn` is our promise, checked by reading `web/gase.js`,
//!   that each of these only reads or writes the memory range it is given
//!   (`ptr .. ptr + len`). With that promise the calls need no `unsafe`.
//!
//! Every value crossing the boundary is a number. Pointers are offsets
//! into the module's linear memory, as `u32` (WebAssembly's `i32`; the
//! page reads them with `>>> 0` to keep them unsigned). Booleans are
//! `0`/`1`. Text and bytes from the page go through the inbox: the page
//! calls [`gase_inbox`] with the length it needs, writes at the returned
//! address, then calls the function that uses them with the length.
//!
//! Everything else is in [`Shell`], which these functions only forward
//! to, so the logic is tested natively.

// See above: `no_mangle` exports and `safe fn` imports, no unsafe blocks.
#![allow(unsafe_code)]

use std::cell::RefCell;

use gase_app::Request;

use crate::{Host, Shell, Written, encode_request};

// --- Imports: what the page provides (the `env` object in gase.js) ------------

#[link(wasm_import_module = "env")]
unsafe extern "C" {
    /// `performance.now()`.
    safe fn now_ms() -> f64;
    /// `console.log` of the UTF-8 text at `ptr`.
    safe fn log(ptr: u32, len: u32);
    /// A panic: show the message and stop the main loop.
    safe fn fatal(ptr: u32, len: u32);
    /// Size of a stored file, or -1 if there is none.
    safe fn file_size(key_ptr: u32, key_len: u32) -> i32;
    /// Copy a stored file into `dst .. dst + dst_len` (at most that much).
    safe fn file_read(key_ptr: u32, key_len: u32, dst: u32, dst_len: u32);
    /// Store (or download) the bytes at `ptr`: 0 stored, 1 downloaded,
    /// -1 failed.
    safe fn file_write(key_ptr: u32, key_len: u32, ptr: u32, len: u32) -> i32;
    /// Stereo frames queued in the audio worklet, or -1 without sound.
    safe fn audio_queued() -> i32;
    /// Queue `len` interleaved `i16` samples from `ptr`.
    safe fn audio_push(ptr: u32, len: u32);
    /// Carry out a request (`crate::REQUEST_*`), with a number and a text.
    safe fn request(kind: u32, arg: u32, ptr: u32, len: u32);
}

/// Address and length of a string or slice, as the page receives them.
fn ptr_len<T>(slice: &[T]) -> (u32, u32) {
    (
        slice.as_ptr().expose_provenance() as u32,
        slice.len() as u32,
    )
}

/// The [`Host`] that calls the imports above.
#[derive(Debug)]
struct JsHost;

impl Host for JsHost {
    fn now_ms(&self) -> f64 {
        now_ms()
    }

    fn log(&mut self, message: &str) {
        let (p, l) = ptr_len(message.as_bytes());
        log(p, l);
    }

    fn read(&mut self, key: &str) -> Option<Vec<u8>> {
        let (kp, kl) = ptr_len(key.as_bytes());
        let size = usize::try_from(file_size(kp, kl)).ok()?;
        let mut data = vec![0u8; size];
        let dst = data.as_mut_ptr().expose_provenance() as u32;
        file_read(kp, kl, dst, size as u32);
        Some(data)
    }

    fn write(&mut self, key: &str, data: &[u8]) -> Written {
        let (kp, kl) = ptr_len(key.as_bytes());
        let (p, l) = ptr_len(data);
        match file_write(kp, kl, p, l) {
            0 => Written::Stored,
            1 => Written::Downloaded,
            _ => Written::Failed,
        }
    }

    fn audio_queued(&self) -> Option<usize> {
        usize::try_from(audio_queued()).ok()
    }

    fn audio_push(&mut self, samples: &[i16]) {
        let (p, l) = ptr_len(samples);
        audio_push(p, l);
    }

    fn request(&mut self, r: &Request) {
        if let Some((kind, arg, text)) = encode_request(r) {
            let (p, l) = ptr_len(text.as_bytes());
            request(kind, arg, p, l);
        }
    }
}

// --- The one shell -------------------------------------------------------------

thread_local! {
    /// A WebAssembly instance without threads has one thread, so this is
    /// simply the program's global state.
    static SHELL: RefCell<Option<Shell<JsHost>>> = const { RefCell::new(None) };
}

/// Run `f` on the shell (or return the default before [`gase_init`]).
fn with<R: Default>(f: impl FnOnce(&mut Shell<JsHost>) -> R) -> R {
    SHELL.with_borrow_mut(|shell| shell.as_mut().map(f).unwrap_or_default())
}

// --- Exports: what the page calls ------------------------------------------------

/// Start: audio at `sample_rate` Hz, capability bits `crate::CAP_*`.
/// Reads the settings, so the page must have loaded storage first.
#[unsafe(no_mangle)]
pub extern "C" fn gase_init(sample_rate: u32, capabilities: u32) {
    // With `panic = "abort"` a panic ends in a WebAssembly trap, which the
    // page sees as a bare "unreachable" error. Send the message first.
    std::panic::set_hook(Box::new(|panic| {
        let text = panic.to_string();
        let (p, l) = ptr_len(text.as_bytes());
        fatal(p, l);
    }));
    let shell = Shell::new(JsHost, sample_rate, capabilities);
    SHELL.with_borrow_mut(|s| *s = Some(shell));
}

/// Make room for `len` bytes from the page; returns where to write them.
#[unsafe(no_mangle)]
pub extern "C" fn gase_inbox(len: u32) -> u32 {
    with(|s| s.inbox(len as usize))
}

/// A key (its `KeyboardEvent.code`, `code_len` bytes in the inbox).
/// Returns 1 if the app uses the key.
#[unsafe(no_mangle)]
pub extern "C" fn gase_key(code_len: u32, pressed: u32, repeat: u32) -> u32 {
    with(|s| u32::from(s.key(code_len as usize, pressed != 0, repeat != 0)))
}

/// A gamepad connected (its `Gamepad.id`, `id_len` bytes in the inbox).
#[unsafe(no_mangle)]
pub extern "C" fn gase_pad_connected(pad: u32, id_len: u32) {
    with(|s| s.pad_connected(pad, id_len as usize));
}

#[unsafe(no_mangle)]
pub extern "C" fn gase_pad_disconnected(pad: u32) {
    with(|s| s.pad_disconnected(pad));
}

/// Standard-mapping button `index` of pad `pad` was pressed or released.
#[unsafe(no_mangle)]
pub extern "C" fn gase_pad_button(pad: u32, index: u32, pressed: u32) {
    with(|s| s.pad_button(pad, index, pressed != 0));
}

/// Axis `index` (0-3 sticks, 4-5 triggers) of pad `pad` moved.
#[unsafe(no_mangle)]
pub extern "C" fn gase_pad_axis(pad: u32, index: u32, value: f32) {
    with(|s| s.pad_axis(pad, index, value));
}

/// A pointer event: `kind` 0 mouse, 1 touch, 2 pen; `phase` 0 down,
/// 1 move, 2 up, 3 cancel; position in physical pixels.
#[unsafe(no_mangle)]
pub extern "C" fn gase_pointer(id: u32, kind: u32, phase: u32, x: f32, y: f32) {
    with(|s| s.pointer(id, kind, phase, x, y));
}

/// A wheel event (`deltaY`, `deltaMode`).
#[unsafe(no_mangle)]
pub extern "C" fn gase_wheel(delta_y: f64, mode: u32) {
    with(|s| s.wheel(delta_y, mode));
}

/// The drawing area changed: physical pixels and `devicePixelRatio`.
#[unsafe(no_mangle)]
pub extern "C" fn gase_resize(width: u32, height: u32, density: f32) {
    with(|s| s.resize(width, height, density));
}

#[unsafe(no_mangle)]
pub extern "C" fn gase_focus_lost() {
    with(Shell::focus_lost);
}

/// The page is hidden: pause and write the saves.
#[unsafe(no_mangle)]
pub extern "C" fn gase_suspend() {
    with(Shell::suspend);
}

#[unsafe(no_mangle)]
pub extern "C" fn gase_fullscreen_changed(on: u32) {
    with(|s| s.fullscreen_changed(on != 0));
}

/// Open the ROM in the inbox: `name_len` bytes of name, then the data.
#[unsafe(no_mangle)]
pub extern "C" fn gase_rom(name_len: u32) {
    with(|s| s.rom(name_len as usize));
}

/// Run one frame; returns the pacing kind (`crate::video::PACE_*`).
#[unsafe(no_mangle)]
pub extern "C" fn gase_update() -> u32 {
    with(Shell::update)
}

/// The address of the frame description (pacing is valid after
/// [`gase_update`], pictures after [`gase_video`]).
#[unsafe(no_mangle)]
pub extern "C" fn gase_info() -> u32 {
    with(|s| s.info_ptr())
}

/// Prepare the pictures; returns the address of the frame description.
#[unsafe(no_mangle)]
pub extern "C" fn gase_video() -> u32 {
    with(Shell::video)
}

/// Emulate `frames` frames flat out (speed measurement).
#[unsafe(no_mangle)]
pub extern "C" fn gase_bench(frames: u32) -> u32 {
    with(|s| s.bench(frames))
}

/// The page is closing: write everything.
#[unsafe(no_mangle)]
pub extern "C" fn gase_shutdown() {
    with(Shell::shutdown);
}
