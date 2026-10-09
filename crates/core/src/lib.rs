//! Sega Mega Drive / Genesis system emulation.
//!
//! This crate wires the individual chips together:
//!
//! * [`gase_m68k`]: the Motorola 68000 main CPU,
//! * [`gase_z80`]: the Zilog Z80 sound CPU,
//! * [`gase_vdp`]: the video display processor,
//! * [`gase_sound`]: the YM2612 FM synthesiser and the SN76489 PSG,
//!
//! and adds what belongs to the console itself: the memory maps
//! ([`bus`]), the cartridge ([`cartridge`]), the controller ports ([`io`]),
//! and the scheduler that runs everything in step ([`system`]).
//! [`debug`] adds breakpoints and single-stepping for debuggers.
//!
//! The crate has no dependencies outside the workspace and no I/O of its
//! own: frontends feed it ROM bytes and button states, and receive frames
//! and audio samples.
//!
//! ```no_run
//! use gase_core::{Cartridge, Config, Genesis};
//!
//! let rom = std::fs::read("game.bin").unwrap();
//! let mut console = Genesis::new(Cartridge::from_bytes(&rom).unwrap(), &Config::default());
//! for _ in 0..60 {
//!     console.run_frame();
//! }
//! let frame = console.frame();
//! println!("{}x{}", frame.width, frame.height);
//! ```

pub mod audio;
pub mod bus;
pub mod cartridge;
pub mod io;
pub mod rewind;
pub mod system;

pub use cartridge::{Cartridge, LoadError};
pub use gase_vdp::{MAX_HEIGHT, MAX_WIDTH, VideoStandard};
pub use io::{Buttons, Device, Region};
pub use rewind::Rewind;
pub use system::debug::{self, Debugger, Stop};
pub use system::{Config, Frame, Genesis, StateError};
