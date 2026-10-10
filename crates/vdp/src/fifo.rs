//! The 4-entry write FIFO between the data port and VDP memory.
//!
//! # Why there is a FIFO
//!
//! The 68000 may write to the data port whenever it likes, but VDP memory
//! can only be written in an external access slot (see `slots`), and during
//! active display those are rare. Rather than stalling the CPU on every
//! write, the VDP queues writes in a small FIFO: each entry remembers the
//! target (the code register), the address and the data, so the CPU can
//! immediately carry on and even change the address for the next write.
//! The address register advances by the auto-increment as each word
//! *enters* the FIFO.
//!
//! One entry leaves the FIFO per slot for CRAM and VSRAM, and per *two*
//! slots for VRAM (the FIFO-to-VRAM path is a byte wide). Four entries is
//! enough to absorb a short burst (a few palette entries in horizontal
//! blanking, say) without the CPU noticing. A fifth write while the FIFO is
//! full stalls the 68000 until a slot frees an entry: in active display that
//! can take a few hundred master clocks per word.
//!
//! The status register shows the FIFO state: bit 9 is set when it is empty,
//! bit 8 when it is full. Careful programs poll bit 9 before reading VRAM
//! or reprogramming the VDP in ways that would affect pending writes.
//!
//! DMA transfers from 68000 memory travel through the same FIFO (see `dma`).

use gase_savestate::{Error, Reader, State, Writer};

/// Number of entries in the FIFO.
pub(crate) const DEPTH: usize = 4;

/// One queued data-port write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The code register when the word was written (CD3-CD0 pick the
    /// memory; the upper bits are kept but ignored).
    pub code: u8,
    pub address: u16,
    pub value: u16,
    /// Access slots still needed before the write is complete.
    pub slots: u8,
    /// This write triggers a VRAM fill once it has been performed.
    pub fill: bool,
}

impl Entry {
    /// A new entry, with the number of slots its target memory needs.
    pub(crate) fn new(code: u8, address: u16, value: u16, fill: bool) -> Self {
        // VRAM takes a slot per byte; CRAM and VSRAM a slot per word.
        // Writes with a read code are discarded but still use a slot.
        let slots = if code & 0x0F == 0x1 { 2 } else { 1 };
        Self {
            code,
            address,
            value,
            slots,
            fill,
        }
    }
}

/// A ring buffer of [`DEPTH`] entries.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fifo {
    entries: [Entry; DEPTH],
    head: usize,
    len: usize,
}

impl Fifo {
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub(crate) fn is_full(&self) -> bool {
        self.len == DEPTH
    }

    /// Add an entry at the back. The caller must make room first.
    pub(crate) fn push(&mut self, entry: Entry) {
        debug_assert!(!self.is_full());
        self.entries[(self.head + self.len) % DEPTH] = entry;
        self.len += 1;
    }

    /// The entry being written to memory, if any.
    pub(crate) fn front_mut(&mut self) -> Option<&mut Entry> {
        if self.is_empty() {
            None
        } else {
            Some(&mut self.entries[self.head])
        }
    }

    /// Remove the front entry. The caller must check it exists.
    pub(crate) fn pop(&mut self) -> Entry {
        debug_assert!(!self.is_empty());
        let entry = self.entries[self.head];
        self.head = (self.head + 1) % DEPTH;
        self.len -= 1;
        entry
    }

    pub(crate) fn clear(&mut self) {
        self.len = 0;
    }
}

impl State for Fifo {
    // Only the live entries are stored, front first.
    fn save(&self, w: &mut Writer) {
        (self.len as u8).save(w);
        for i in 0..self.len {
            let e = &self.entries[(self.head + i) % DEPTH];
            e.code.save(w);
            e.address.save(w);
            e.value.save(w);
            e.slots.save(w);
            e.fill.save(w);
        }
    }

    fn load(&mut self, r: &mut Reader<'_>) -> Result<(), Error> {
        let mut len = 0u8;
        len.load(r)?;
        if usize::from(len) > DEPTH {
            return Err(Error::Invalid("VDP FIFO length"));
        }
        self.head = 0;
        self.len = 0;
        for _ in 0..len {
            let mut e = Entry::default();
            e.code.load(r)?;
            e.address.load(r)?;
            e.value.load(r)?;
            e.slots.load(r)?;
            e.fill.load(r)?;
            self.push(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_order_and_flags() {
        let mut fifo = Fifo::default();
        assert!(fifo.is_empty());
        for i in 0..DEPTH as u16 {
            fifo.push(Entry::new(3, i * 2, i, false));
        }
        assert!(fifo.is_full());
        assert_eq!(fifo.pop().value, 0);
        fifo.push(Entry::new(3, 8, 4, false));
        let values: Vec<u16> = (0..DEPTH).map(|_| fifo.pop().value).collect();
        assert_eq!(values, [1, 2, 3, 4]);
        assert!(fifo.is_empty());
    }

    #[test]
    fn vram_entries_need_two_slots() {
        assert_eq!(Entry::new(0x01, 0, 0, false).slots, 2);
        assert_eq!(Entry::new(0x21, 0, 0, false).slots, 2);
        assert_eq!(Entry::new(0x03, 0, 0, false).slots, 1);
        assert_eq!(Entry::new(0x05, 0, 0, false).slots, 1);
    }

    #[test]
    fn save_state_round_trip() {
        let mut fifo = Fifo::default();
        fifo.push(Entry::new(1, 0x10, 0xAAAA, false));
        fifo.push(Entry::new(1, 0x12, 0xBBBB, true));
        fifo.pop();
        fifo.push(Entry::new(3, 0x02, 0x0E0E, false));
        let mut w = Writer::new();
        fifo.save(&mut w);
        let bytes = w.into_bytes();
        let mut copy = Fifo::default();
        copy.load(&mut Reader::new(&bytes)).unwrap();
        assert_eq!(copy.len(), 2);
        assert_eq!(copy.pop(), Entry::new(1, 0x12, 0xBBBB, true));
        assert_eq!(copy.pop(), Entry::new(3, 0x02, 0x0E0E, false));
    }
}
