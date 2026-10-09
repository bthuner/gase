//! The VDP's notion of time: draining the FIFO and running DMA, one access
//! slot at a time.
//!
//! # Catching up
//!
//! The VDP keeps a position, `pos`, in master clocks from the start of the
//! current line: every access slot before `pos` has been used. The system
//! calls [`Vdp::advance`] with the current time before each port access and
//! [`Vdp::begin_line`] finishes the line, so the VDP only does work when
//! somebody could observe it. When nothing is queued (the usual case)
//! `advance` just moves `pos` forward, so this costs nearly nothing.
//!
//! In each slot the VDP performs one step of whichever job is active:
//!
//! 1. a running **VRAM fill** or **VRAM copy**, otherwise
//! 2. the write at the front of the **FIFO** (see `fifo`), and, if a
//!    **68000-to-VDP DMA** is feeding the FIFO, the next source word takes
//!    the freed entry.
//!
//! # The three kinds of DMA
//!
//! * **68000 to VDP.** The VDP becomes the 68000 bus master and reads words
//!   from ROM or RAM into the FIFO, which drains at slot speed. The 68000 is
//!   frozen from the moment it writes the command until the last word has
//!   entered the FIFO (the last four may still be draining when it wakes
//!   up). Since a VRAM word needs two slots, a transfer during active H40
//!   display moves about 9 words per line, against about 102 in blanking: a
//!   4 KiB tile upload in active display would freeze the game for most of a
//!   frame, but fits comfortably in vertical blanking. Hence DMA queues
//!   flushed from the vertical interrupt, and display-off loading screens.
//! * **VRAM fill.** Armed by the command, started by the data-port write
//!   that follows (that write itself is performed normally first). Then one
//!   byte per slot is written at `address ^ 1`, the address advancing by
//!   the auto-increment. The 68000 keeps running; it can watch status bit 1
//!   (DMA busy) to know when the fill is over.
//! * **VRAM copy.** Copies bytes within VRAM. Each byte needs a read slot
//!   and a write slot, so it runs at half the fill rate. The 68000 also
//!   keeps running.
//!
//! While any of them runs, the length registers (19-20) count down and the
//! source registers (21-22) count up, so they read back the progress, and
//! after the DMA the source points just past the transferred data, which
//! some games rely on to chain transfers.
//!
//! # What the 68000 may do during a fill or copy
//!
//! The 68000 is not frozen, and register writes take effect at once: the
//! auto-increment and the length registers are re-read at every step, as on
//! hardware. Following the commonly documented behaviour, a data-port write
//! during a fill replaces the fill value rather than being written to
//! memory. Data-port writes during a copy wait in the FIFO until the copy is
//! over (stalling the 68000 if it fills up), and data-port reads wait for
//! both the FIFO and any fill or copy to finish.

use gase_savestate::{Error, Reader, State, Writer};

use crate::Vdp;
use crate::fifo::Entry;
use crate::slots::SlotMode;
use crate::timing::MASTER_CYCLES_PER_LINE as LINE;

/// The DMA operation in progress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Dma {
    #[default]
    Idle,
    /// A fill command has been written; the next data-port write starts it.
    FillArmed,
    /// The data-port write that starts the fill is still in the FIFO.
    FillWait,
    /// Filling, one byte (or CRAM/VSRAM word) per slot.
    Fill,
    /// Copying within VRAM, one byte per two slots.
    Copy,
    /// 68000 memory is being transferred through the FIFO.
    Memory,
}

impl Dma {
    fn to_u8(self) -> u8 {
        match self {
            Dma::Idle => 0,
            Dma::FillArmed => 1,
            Dma::FillWait => 2,
            Dma::Fill => 3,
            Dma::Copy => 4,
            Dma::Memory => 5,
        }
    }

    fn from_u8(value: u8) -> Result<Self, Error> {
        Ok(match value {
            0 => Dma::Idle,
            1 => Dma::FillArmed,
            2 => Dma::FillWait,
            3 => Dma::Fill,
            4 => Dma::Copy,
            5 => Dma::Memory,
            _ => return Err(Error::Invalid("VDP DMA state")),
        })
    }
}

/// Everything the slot engine needs beyond the port registers.
#[derive(Clone, Debug, Default)]
pub(crate) struct Engine {
    /// Master clocks from the start of the current line up to which every
    /// slot has been used. May exceed the line length after a CPU stall that
    /// crossed into the next line.
    pub pos: u32,
    pub dma: Dma,
    /// Target memory (code register) of the running fill.
    pub dma_code: u8,
    /// The value being filled (its high byte for VRAM).
    pub fill_value: u16,
    /// VRAM copy: the byte read in the first slot, written in the second.
    pub copy_latch: Option<u8>,
    /// A 68000 DMA has been requested and its source words must be fetched
    /// by the system with [`Vdp::fetch_dma_source`].
    pub source_needed: bool,
    /// Source words of the running 68000 DMA (fetched when it starts: the
    /// 68000 is frozen meanwhile, so its memory cannot change).
    pub words: Vec<u16>,
    /// Next word of `words` to enter the FIFO.
    pub next_word: usize,
    /// Line position at which the last 68000 DMA word entered the FIFO.
    pub memory_done_at: u32,
    /// Master clocks the CPU must wait because of its last port access.
    pub cpu_stall: u32,
}

impl State for Engine {
    fn save(&self, w: &mut Writer) {
        self.pos.save(w);
        self.dma.to_u8().save(w);
        self.dma_code.save(w);
        self.fill_value.save(w);
        self.copy_latch.is_some().save(w);
        self.copy_latch.unwrap_or(0).save(w);
        self.source_needed.save(w);
        // Only the words still to be transferred.
        let rest = &self.words[self.next_word.min(self.words.len())..];
        (rest.len() as u32).save(w);
        for word in rest {
            word.save(w);
        }
        self.memory_done_at.save(w);
    }

    fn load(&mut self, r: &mut Reader<'_>) -> Result<(), Error> {
        self.pos.load(r)?;
        let mut dma = 0u8;
        dma.load(r)?;
        self.dma = Dma::from_u8(dma)?;
        self.dma_code.load(r)?;
        self.fill_value.load(r)?;
        let (mut latched, mut latch) = (false, 0u8);
        latched.load(r)?;
        latch.load(r)?;
        self.copy_latch = latched.then_some(latch);
        self.source_needed.load(r)?;
        let mut len = 0u32;
        len.load(r)?;
        if len > 0x1_0000 {
            return Err(Error::Invalid("VDP DMA length"));
        }
        self.words.clear();
        for _ in 0..len {
            let mut word = 0u16;
            word.load(r)?;
            self.words.push(word);
        }
        self.next_word = 0;
        self.memory_done_at.load(r)?;
        self.cpu_stall = 0;
        Ok(())
    }
}

impl Vdp {
    /// The slot pattern of the current line.
    #[inline]
    pub(crate) fn slot_mode(&self) -> SlotMode {
        SlotMode::new(self.h40(), self.in_vblank || !self.display_enabled())
    }

    /// Number of external access slots in a line with the current settings
    /// (18 or 16 during active display, 205 or 167 during blanking).
    #[must_use]
    pub fn access_slots_per_line(&self) -> u32 {
        self.slot_mode().per_line()
    }

    /// Number of writes waiting in the FIFO (0 to 4).
    #[must_use]
    pub fn fifo_len(&self) -> usize {
        self.fifo.len()
    }

    /// Nothing needs a slot.
    #[inline]
    fn idle(&self) -> bool {
        self.fifo.is_empty() && matches!(self.engine.dma, Dma::Idle | Dma::FillArmed)
    }

    /// Run the VDP up to `line_cycle` master clocks into the current line,
    /// performing queued writes and DMA steps in the access slots on the way.
    pub fn advance(&mut self, line_cycle: u32) {
        if self.engine.pos >= line_cycle {
            return;
        }
        // Slot jobs never touch the registers that select the pattern.
        let mode = self.slot_mode();
        while !self.idle() {
            let slot = mode.next_slot(self.engine.pos);
            if slot >= line_cycle {
                break;
            }
            self.use_slot(slot);
        }
        self.engine.pos = line_cycle;
    }

    /// Wait for the next slot and use it; `pos` moves just past the slot.
    pub(crate) fn step_slot(&mut self) {
        let slot = self.slot_mode().next_slot(self.engine.pos);
        self.use_slot(slot);
    }

    #[inline]
    fn use_slot(&mut self, slot: u32) {
        self.engine.pos = slot + 1;
        match self.engine.dma {
            Dma::Fill => return self.fill_step(),
            Dma::Copy => return self.copy_step(),
            _ => {}
        }
        let Some(front) = self.fifo.front_mut() else {
            return;
        };
        front.slots -= 1;
        if front.slots > 0 {
            return;
        }
        let entry = self.fifo.pop();
        self.write_memory(entry.code, entry.address, entry.value);
        if entry.fill && self.engine.dma == Dma::FillWait {
            self.engine.dma = Dma::Fill;
            self.engine.dma_code = entry.code;
        }
        if self.engine.dma == Dma::Memory {
            self.feed_fifo(slot);
        }
    }

    /// Run the CPU-side clock forward until `done` holds, charging the time
    /// to the CPU as a stall.
    pub(crate) fn stall_until(&mut self, mut done: impl FnMut(&Self) -> bool) {
        let start = self.engine.pos;
        while !done(self) {
            self.step_slot();
        }
        self.engine.cpu_stall += self.engine.pos - start;
    }

    /// Master clocks the CPU had to wait during its port accesses since the
    /// last call (FIFO full, or a data-port read waiting for the FIFO).
    pub fn take_cpu_stall(&mut self) -> u32 {
        std::mem::take(&mut self.engine.cpu_stall)
    }

    /// Finish the current line and make the next one current.
    pub(crate) fn next_line_slots(&mut self) {
        self.advance(LINE);
        self.engine.pos -= LINE;
    }

    // --- DMA ---------------------------------------------------------------

    pub(crate) fn dma_length(&self) -> u32 {
        match u32::from(self.regs[19]) | u32::from(self.regs[20]) << 8 {
            0 => 0x1_0000,
            n => n,
        }
    }

    /// Count one unit off the length registers. Returns true when the DMA
    /// is complete. A length of 0 means 65536, so the check happens after
    /// the decrement.
    fn count_down(&mut self) -> bool {
        let length = (u16::from(self.regs[19]) | u16::from(self.regs[20]) << 8).wrapping_sub(1);
        self.regs[19] = length as u8;
        self.regs[20] = (length >> 8) as u8;
        length == 0
    }

    fn source(&self) -> u16 {
        u16::from(self.regs[21]) | u16::from(self.regs[22]) << 8
    }

    /// Advance the low 16 bits of the source. Register 23 is never
    /// incremented, so 68000 DMA wraps within a 128 KiB window.
    fn increment_source(&mut self) {
        let source = self.source().wrapping_add(1);
        self.regs[21] = source as u8;
        self.regs[22] = (source >> 8) as u8;
    }

    pub(crate) fn start_dma(&mut self) {
        self.engine.copy_latch = None;
        match self.regs[23] >> 6 {
            0 | 1 => self.engine.source_needed = true,
            2 => self.engine.dma = Dma::FillArmed,
            _ => self.engine.dma = Dma::Copy,
        }
    }

    /// Is a 68000-to-VDP DMA waiting for its source data?
    #[must_use]
    pub fn dma_68k_pending(&self) -> bool {
        self.engine.source_needed
    }

    /// Is a 68000-to-VDP DMA running? The 68000 is frozen until it ends.
    #[must_use]
    pub fn dma_68k_active(&self) -> bool {
        self.engine.source_needed || self.engine.dma == Dma::Memory
    }

    /// Line position (master clocks from the start of the line) at which
    /// the last 68000 DMA word entered the FIFO, i.e. when the 68000 may run
    /// again.
    #[must_use]
    pub fn dma_68k_done_at(&self) -> u32 {
        self.engine.memory_done_at
    }

    /// Is any DMA operation in progress (status bit 1)?
    #[must_use]
    pub fn dma_busy(&self) -> bool {
        self.engine.source_needed
            || matches!(
                self.engine.dma,
                Dma::FillWait | Dma::Fill | Dma::Copy | Dma::Memory
            )
    }

    /// Start a pending 68000-to-VDP DMA, reading its source words through
    /// `read` (a 68000 byte address; only the system bus can read 68000
    /// memory). The transfer then runs through the FIFO as time advances.
    pub fn fetch_dma_source(&mut self, mut read: impl FnMut(u32) -> u16) {
        if !self.engine.source_needed {
            return;
        }
        self.engine.source_needed = false;
        let length = self.dma_length();
        let high = u32::from(self.regs[23] & 0x7F) << 17;
        let mut source = self.source();
        self.engine.words.clear();
        for _ in 0..length {
            self.engine
                .words
                .push(read(high | (u32::from(source) << 1)));
            source = source.wrapping_add(1);
        }
        self.engine.next_word = 0;
        self.engine.dma = Dma::Memory;
        // The VDP grabs the bus straight away and fills the free entries.
        self.feed_fifo(self.engine.pos);
    }

    /// Move 68000 DMA words into the FIFO while there is room.
    fn feed_fifo(&mut self, now: u32) {
        while !self.fifo.is_full() {
            let Some(&word) = self.engine.words.get(self.engine.next_word) else {
                break;
            };
            self.engine.next_word += 1;
            self.fifo
                .push(Entry::new(self.code, self.address, word, false));
            self.address = self.address.wrapping_add(self.auto_increment());
            self.increment_source();
            self.count_down();
        }
        if self.engine.next_word >= self.engine.words.len() {
            self.engine.dma = Dma::Idle;
            self.engine.memory_done_at = now;
            self.engine.words.clear();
            self.engine.next_word = 0;
        }
    }

    fn fill_step(&mut self) {
        let value = self.engine.fill_value;
        match self.engine.dma_code & 0x0F {
            // VRAM fill writes the high byte next to the current address.
            0x1 => self.write_vram_byte(self.address ^ 1, (value >> 8) as u8),
            // CRAM and VSRAM fills write the whole word.
            code @ (0x3 | 0x5) => self.write_memory(code, self.address, value),
            _ => {}
        }
        self.address = self.address.wrapping_add(self.auto_increment());
        if self.count_down() {
            self.engine.dma = Dma::Idle;
        }
    }

    fn copy_step(&mut self) {
        match self.engine.copy_latch.take() {
            None => {
                self.engine.copy_latch = Some(self.vram[usize::from(self.source())]);
            }
            Some(byte) => {
                self.write_vram_byte(self.address, byte);
                self.increment_source();
                self.address = self.address.wrapping_add(self.auto_increment());
                if self.count_down() {
                    self.engine.dma = Dma::Idle;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::timing::MASTER_CYCLES_PER_LINE as LINE;
    use crate::{Vdp, VideoStandard};

    fn command(vdp: &mut Vdp, code: u8, addr: u16) {
        let code = u16::from(code);
        vdp.write_control(((code & 3) << 14) | (addr & 0x3FFF));
        vdp.write_control(((code & 0x3C) << 2) | (addr >> 14));
    }

    /// A VDP in H40 with the display on, on active line 10.
    fn active_h40() -> Vdp {
        let mut vdp = Vdp::new(VideoStandard::Ntsc);
        vdp.write_control(0x8154); // display on, DMA enabled, mode 5
        vdp.write_control(0x8C81); // H40
        vdp.write_control(0x8F02);
        vdp.begin_line(10);
        vdp
    }

    /// The same, but on the first line of vertical blanking.
    fn blank_h40() -> Vdp {
        let mut vdp = active_h40();
        vdp.begin_line(224);
        vdp
    }

    /// Run whole lines until `done`, returning how many it took.
    fn lines_until(vdp: &mut Vdp, first: u16, mut done: impl FnMut(&Vdp) -> bool) -> u32 {
        let mut lines = 0;
        let mut line = first;
        while !done(vdp) {
            vdp.advance(LINE);
            line = (line + 1) % vdp.total_lines();
            vdp.begin_line(line);
            lines += 1;
            assert!(lines < 10_000, "never finished");
        }
        lines
    }

    #[test]
    fn writes_wait_in_the_fifo_until_a_slot() {
        let mut vdp = active_h40();
        command(&mut vdp, 0x03, 0); // CRAM: one slot per word
        vdp.write_data(0x0EEE);
        assert_eq!(vdp.cram[0], 0, "not written before the first slot");
        assert_eq!(vdp.read_status(0) & 0x0300, 0, "neither empty nor full");
        vdp.advance(104);
        assert_eq!(vdp.cram[0], 0, "slot at 104 not reached yet");
        vdp.advance(105);
        assert_eq!(vdp.cram[0], 0x0EEE);
        assert_eq!(vdp.read_status(0) & 0x0200, 0x0200, "empty again");
    }

    #[test]
    fn fifo_full_flag_and_cpu_stall() {
        let mut vdp = active_h40();
        command(&mut vdp, 0x01, 0x1000);
        for i in 1..=4 {
            vdp.write_data(i);
        }
        assert_eq!(vdp.read_status(0) & 0x0300, 0x0100, "full, not empty");
        assert_eq!(vdp.fifo_len(), 4);
        assert_eq!(vdp.take_cpu_stall(), 0, "four writes fit");

        // The fifth write must wait until the first entry, a VRAM word,
        // has had its two slots (104 and 232).
        vdp.write_data(5);
        assert_eq!(vdp.take_cpu_stall(), 233);
        assert_eq!(vdp.vram_word(0x1000), 1);
        assert_eq!(vdp.vram_word(0x1002), 0);
        assert_eq!(vdp.read_status(0) & 0x0100, 0x0100);

        // Five VRAM words take ten slots: all done after slot 1640.
        vdp.advance(1640);
        assert_ne!(vdp.read_status(0) & 0x0200, 0x0200);
        vdp.advance(1641);
        assert_eq!(vdp.read_status(0) & 0x0200, 0x0200);
        for i in 0..5 {
            assert_eq!(vdp.vram_word(0x1000 + i * 2), i + 1);
        }
    }

    #[test]
    fn fifo_drains_much_faster_in_blanking() {
        let mut vdp = blank_h40();
        command(&mut vdp, 0x01, 0);
        for i in 0..4 {
            vdp.write_data(i);
        }
        // Eight slots at 3420/205 ≈ 16.7 clocks apart.
        vdp.advance(8 * LINE / 205 + 1);
        assert_eq!(vdp.read_status(0) & 0x0200, 0x0200);
    }

    #[test]
    fn data_port_read_waits_for_the_fifo() {
        let mut vdp = active_h40();
        command(&mut vdp, 0x01, 0x2000);
        vdp.write_data(0x1234);
        command(&mut vdp, 0x00, 0x2000);
        assert_eq!(vdp.read_data(), 0x1234);
        // Two slots for the write, one more for the read.
        assert_eq!(vdp.take_cpu_stall(), 361);
    }

    #[test]
    fn dma_68k_progress_and_registers() {
        let mut vdp = active_h40();
        vdp.write_control(0x9314); // 20 words
        vdp.write_control(0x9400);
        vdp.write_control(0x9580); // source word 0x000080 = byte 0x000100
        vdp.write_control(0x9600);
        vdp.write_control(0x9700);
        command(&mut vdp, 0x21, 0x4000);
        assert!(vdp.dma_68k_pending() && vdp.dma_68k_active() && vdp.dma_busy());
        vdp.fetch_dma_source(|addr| addr as u16);
        // Four words went straight into the FIFO.
        assert!(vdp.dma_68k_active());
        assert_eq!(vdp.regs[19], 16);
        assert_eq!(vdp.regs[21], 0x84);
        assert_eq!(vdp.read_status(0) & 0x0102, 0x0102, "FIFO full, DMA busy");

        // Halfway through the line: 7 slots used (104..872), so 3 entries
        // have left and 3 more words entered.
        vdp.advance(1000);
        assert_eq!(vdp.regs[19], 13);
        assert_eq!(vdp.regs[21], 0x87);
        assert_eq!(vdp.vram_word(0x4000), 0x100);
        assert_eq!(vdp.vram_word(0x4004), 0x104);
        assert_eq!(vdp.vram_word(0x4006), 0);

        // 20 words need 40 slots; the 68000 is released when the last one
        // has entered the FIFO, 32 slots in.
        // That happens during line 11.
        let lines = lines_until(&mut vdp, 10, |v| !v.dma_68k_active());
        assert_eq!(lines, 2);
        assert!(!vdp.dma_busy());
        assert_eq!(vdp.regs[19], 0);
        assert_eq!(vdp.regs[20], 0);
        assert_eq!(vdp.regs[21], 0x94, "source points past the data");
        // 36 slots so far: two entries are still draining.
        assert_eq!(vdp.read_status(0) & 0x0200, 0, "last words still draining");
        vdp.advance(LINE);
        assert_eq!(vdp.read_status(0) & 0x0200, 0x0200);
        for i in 0..20 {
            assert_eq!(vdp.vram_word(0x4000 + i * 2), 0x100 + i * 2);
        }
    }

    #[test]
    fn dma_68k_to_cram_uses_one_slot_per_word() {
        let mut vdp = active_h40();
        vdp.write_control(0x9312); // 18 words: exactly one line of slots
        vdp.write_control(0x9400);
        vdp.write_control(0x9700);
        command(&mut vdp, 0x23, 0);
        vdp.fetch_dma_source(|_| 0x0222);
        vdp.advance(LINE);
        assert!(!vdp.dma_68k_active());
        // The last word entered when the 14th entry left, at the 14th slot.
        assert_eq!(vdp.dma_68k_done_at(), 2280);
        assert!(vdp.cram[..18].iter().all(|&c| c == 0x0222));
    }

    #[test]
    fn dma_68k_bandwidth_active_vs_blanking() {
        // 900 VRAM words: ~9 per active line, ~102 per blanking line.
        let run = |mut vdp: Vdp, line: u16| {
            vdp.write_control(0x9384);
            vdp.write_control(0x9403);
            vdp.write_control(0x9700);
            command(&mut vdp, 0x21, 0);
            vdp.fetch_dma_source(|_| 0xFFFF);
            lines_until(&mut vdp, line, |v| !v.dma_68k_active())
        };
        let active = run(active_h40(), 10);
        // 896 words after the first four, two slots each, 18 slots a line.
        assert!((99..=101).contains(&active), "{active} lines");
        let mut blank = blank_h40();
        blank.write_control(0x8114);
        let display_off = run(blank, 224);
        assert!((8..=10).contains(&display_off), "{display_off} lines");
    }

    #[test]
    fn fill_runs_over_time_with_busy_flag() {
        let mut vdp = blank_h40();
        vdp.write_control(0x8F01);
        vdp.write_control(0x9300); // 0x0400 bytes
        vdp.write_control(0x9404);
        vdp.write_control(0x9780);
        command(&mut vdp, 0x21, 0x8000);
        assert_eq!(vdp.read_status(0) & 0x02, 0, "armed, not busy yet");
        vdp.write_data(0xAB00);
        assert_eq!(vdp.read_status(0) & 0x02, 0x02);
        assert_eq!(vdp.take_cpu_stall(), 0, "the 68000 is not frozen");
        // 2 slots for the first write then 1024 one-byte slots: 1026 slots
        // at 205 per line ends one slot into the sixth line.
        let lines = lines_until(&mut vdp, 224, |v| !v.dma_busy());
        assert_eq!(lines, 6);
        assert!(vdp.vram[0x8002..0x8400].iter().all(|&b| b == 0xAB));
        assert_eq!(vdp.regs[19], 0);
        assert_eq!(vdp.regs[20], 0);
        assert_eq!(vdp.vram[0x8401], 0xAB);
        assert_eq!(vdp.vram[0x8402], 0);
    }

    #[test]
    fn fill_in_active_display_is_slow() {
        let mut vdp = active_h40();
        vdp.write_control(0x8F01);
        vdp.write_control(0x9346); // 70 bytes + 2 slots for the first write: 4 lines
        vdp.write_control(0x9400);
        vdp.write_control(0x9780);
        command(&mut vdp, 0x21, 0x8000);
        vdp.write_data(0xAB00);
        let lines = lines_until(&mut vdp, 10, |v| !v.dma_busy());
        assert_eq!(lines, 4);
    }

    #[test]
    fn data_write_during_fill_changes_the_value() {
        let mut vdp = blank_h40();
        vdp.write_control(0x8F01);
        vdp.write_control(0x9300);
        vdp.write_control(0x9402); // 512 bytes
        vdp.write_control(0x9780);
        command(&mut vdp, 0x21, 0x8000);
        vdp.write_data(0x1100);
        vdp.advance(LINE / 2);
        vdp.write_data(0x2200);
        lines_until(&mut vdp, 224, |v| !v.dma_busy());
        assert_eq!(vdp.vram[0x8010], 0x11);
        assert_eq!(vdp.vram[0x81F0], 0x22);
    }

    #[test]
    fn copy_runs_at_half_the_fill_rate() {
        let mut vdp = blank_h40();
        for i in 0..0x200 {
            vdp.vram[0x1000 + i] = i as u8;
        }
        vdp.write_control(0x8F01);
        vdp.write_control(0x9300); // 0x200 bytes
        vdp.write_control(0x9402);
        vdp.write_control(0x9500); // source 0x1000
        vdp.write_control(0x9610);
        vdp.write_control(0x97C0);
        command(&mut vdp, 0x20, 0x3000);
        assert!(vdp.dma_busy());
        assert_eq!(vdp.take_cpu_stall(), 0);
        // Halfway through the first line: ~51 bytes copied.
        vdp.advance(LINE / 2);
        let done = 0x200 - (usize::from(vdp.regs[19]) | usize::from(vdp.regs[20]) << 8);
        assert!((50..=53).contains(&done), "{done} bytes");
        assert_eq!(usize::from(vdp.regs[21]), done, "source follows");
        // 1024 slots: 5 lines.
        let lines = lines_until(&mut vdp, 224, |v| !v.dma_busy());
        assert_eq!(lines, 5);
        assert_eq!(&vdp.vram[0x3000..0x3200], &vdp.vram[0x1000..0x1200]);
        assert_eq!(vdp.regs[22], 0x12);
    }

    #[test]
    fn writes_during_copy_wait_for_it() {
        let mut vdp = active_h40();
        vdp.write_control(0x9310);
        vdp.write_control(0x9400);
        vdp.write_control(0x97C0);
        command(&mut vdp, 0x20, 0x3000);
        command(&mut vdp, 0x03, 0);
        for _ in 0..5 {
            vdp.write_data(0x0E00);
        }
        // The fifth write waited for the 32-slot copy plus one CRAM slot.
        let stall = vdp.take_cpu_stall();
        assert!(stall > LINE, "{stall}");
        assert!(!vdp.dma_busy());
    }

    #[test]
    fn stall_crossing_the_line_end_carries_over() {
        let mut vdp = active_h40();
        vdp.advance(3300);
        command(&mut vdp, 0x01, 0);
        for i in 0..5 {
            vdp.write_data(i);
        }
        // The next slots are at 104 and 232 of the next line.
        assert_eq!(vdp.take_cpu_stall(), LINE + 233 - 3300);
        vdp.begin_line(11);
        vdp.advance(233);
        assert_eq!(vdp.vram_word(0), 0);
        vdp.advance(LINE);
        assert_eq!(vdp.read_status(0) & 0x200, 0x200);
        assert_eq!(vdp.vram_word(8), 4);
    }

    #[test]
    fn save_state_mid_dma() {
        use gase_savestate::{Reader, State, Writer};
        let mut vdp = active_h40();
        vdp.write_control(0x9364);
        vdp.write_control(0x9400);
        vdp.write_control(0x9700);
        command(&mut vdp, 0x21, 0);
        vdp.fetch_dma_source(|addr| addr as u16);
        vdp.advance(2000);
        let mut w = Writer::new();
        vdp.save(&mut w);
        let bytes = w.into_bytes();
        let mut copy = Vdp::new(VideoStandard::Ntsc);
        copy.load(&mut Reader::new(&bytes)).unwrap();
        let a = lines_until(&mut vdp, 10, |v| !v.dma_68k_active());
        let b = lines_until(&mut copy, 10, |v| !v.dma_68k_active());
        assert_eq!(a, b);
        assert_eq!(vdp.vram, copy.vram);
        assert_eq!(vdp.regs, copy.regs);
    }
}
