//! A tiny, dependency-free binary serialisation format for save states.
//!
//! Every emulated component implements [`State`]: it writes its fields, in a
//! fixed order, into a [`Writer`] and reads them back from a [`Reader`].
//! There is no self-description, no field names and no versioning per field;
//! the whole save state carries a single format version instead (see
//! `gase-core`). This keeps the format trivial to understand and fast.
//!
//! All integers are little-endian.
//!
//! ```
//! use gase_savestate::{State, Writer, Reader};
//!
//! #[derive(Default, PartialEq, Debug)]
//! struct Counter { value: u32, enabled: bool, regs: [u8; 4] }
//!
//! impl State for Counter {
//!     fn save(&self, w: &mut Writer) {
//!         self.value.save(w);
//!         self.enabled.save(w);
//!         self.regs.save(w);
//!     }
//!     fn load(&mut self, r: &mut Reader<'_>) -> Result<(), gase_savestate::Error> {
//!         self.value.load(r)?;
//!         self.enabled.load(r)?;
//!         self.regs.load(r)
//!     }
//! }
//!
//! let a = Counter { value: 42, enabled: true, regs: [1, 2, 3, 4] };
//! let mut w = Writer::new();
//! a.save(&mut w);
//! let bytes = w.into_bytes();
//!
//! let mut b = Counter::default();
//! b.load(&mut Reader::new(&bytes)).unwrap();
//! assert_eq!(a, b);
//! ```

use std::fmt;

/// Error produced when loading a save state fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The input ended before all fields were read.
    UnexpectedEof,
    /// A value was read that is not valid for its field (e.g. a bool of 7).
    Invalid(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnexpectedEof => f.write_str("save state is truncated"),
            Error::Invalid(what) => write!(f, "save state contains an invalid {what}"),
        }
    }
}

impl std::error::Error for Error {}

/// Accumulates serialised bytes.
#[derive(Debug, Default, Clone)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// Create an empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append raw bytes.
    pub fn bytes(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Consume the writer and return the serialised bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

/// Reads serialised bytes back.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Create a reader over `data`.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// Take exactly `n` raw bytes.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.data.len() < n {
            return Err(Error::UnexpectedEof);
        }
        let (head, tail) = self.data.split_at(n);
        self.data = tail;
        Ok(head)
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.data.len()
    }
}

/// Something that can be written to and restored from a save state.
pub trait State {
    /// Serialise `self` into `w`.
    fn save(&self, w: &mut Writer);
    /// Overwrite `self` with data read from `r`.
    fn load(&mut self, r: &mut Reader<'_>) -> Result<(), Error>;
}

macro_rules! impl_int {
    ($($t:ty),*) => {$(
        impl State for $t {
            #[inline]
            fn save(&self, w: &mut Writer) {
                w.bytes(&self.to_le_bytes());
            }
            #[inline]
            fn load(&mut self, r: &mut Reader<'_>) -> Result<(), Error> {
                let b = r.bytes(size_of::<$t>())?;
                *self = <$t>::from_le_bytes(b.try_into().expect("length checked"));
                Ok(())
            }
        }
    )*};
}

impl_int!(u8, u16, u32, u64, i8, i16, i32, i64);

impl State for usize {
    fn save(&self, w: &mut Writer) {
        (*self as u64).save(w);
    }
    fn load(&mut self, r: &mut Reader<'_>) -> Result<(), Error> {
        let mut v = 0u64;
        v.load(r)?;
        *self = usize::try_from(v).map_err(|_| Error::Invalid("usize"))?;
        Ok(())
    }
}

impl State for f32 {
    fn save(&self, w: &mut Writer) {
        self.to_bits().save(w);
    }
    fn load(&mut self, r: &mut Reader<'_>) -> Result<(), Error> {
        let mut bits = 0u32;
        bits.load(r)?;
        *self = f32::from_bits(bits);
        Ok(())
    }
}

impl State for f64 {
    fn save(&self, w: &mut Writer) {
        self.to_bits().save(w);
    }
    fn load(&mut self, r: &mut Reader<'_>) -> Result<(), Error> {
        let mut bits = 0u64;
        bits.load(r)?;
        *self = f64::from_bits(bits);
        Ok(())
    }
}

impl State for bool {
    fn save(&self, w: &mut Writer) {
        u8::from(*self).save(w);
    }
    fn load(&mut self, r: &mut Reader<'_>) -> Result<(), Error> {
        match r.bytes(1)?[0] {
            0 => *self = false,
            1 => *self = true,
            _ => return Err(Error::Invalid("bool")),
        }
        Ok(())
    }
}

impl<T: State, const N: usize> State for [T; N] {
    fn save(&self, w: &mut Writer) {
        for item in self {
            item.save(w);
        }
    }
    fn load(&mut self, r: &mut Reader<'_>) -> Result<(), Error> {
        for item in self {
            item.load(r)?;
        }
        Ok(())
    }
}

/// Vectors store their length, and must load back with the same length:
/// emulated memories never change size at runtime, so a mismatch means the
/// state belongs to a different configuration.
impl<T: State> State for Vec<T> {
    fn save(&self, w: &mut Writer) {
        self.len().save(w);
        for item in self {
            item.save(w);
        }
    }
    fn load(&mut self, r: &mut Reader<'_>) -> Result<(), Error> {
        let mut len = 0usize;
        len.load(r)?;
        if len != self.len() {
            return Err(Error::Invalid("buffer length"));
        }
        for item in self {
            item.load(r)?;
        }
        Ok(())
    }
}

/// Implement [`State`] for a struct by listing its fields in order.
///
/// ```
/// #[derive(Default)]
/// struct Timer { counter: u16, reload: u16, running: bool }
/// gase_savestate::impl_state!(Timer { counter, reload, running });
/// ```
#[macro_export]
macro_rules! impl_state {
    ($ty:ty { $($field:ident),* $(,)? }) => {
        impl $crate::State for $ty {
            fn save(&self, w: &mut $crate::Writer) {
                $( $crate::State::save(&self.$field, w); )*
            }
            fn load(&mut self, r: &mut $crate::Reader<'_>) -> Result<(), $crate::Error> {
                $( $crate::State::load(&mut self.$field, r)?; )*
                Ok(())
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_primitives() {
        let mut w = Writer::new();
        0xDEAD_BEEFu32.save(&mut w);
        (-5i16).save(&mut w);
        true.save(&mut w);
        [1u8, 2, 3].save(&mut w);
        vec![7u16; 4].save(&mut w);
        let bytes = w.into_bytes();

        let mut r = Reader::new(&bytes);
        let (mut a, mut b, mut c, mut d, mut e) = (0u32, 0i16, false, [0u8; 3], vec![0u16; 4]);
        a.load(&mut r).unwrap();
        b.load(&mut r).unwrap();
        c.load(&mut r).unwrap();
        d.load(&mut r).unwrap();
        e.load(&mut r).unwrap();
        assert_eq!(
            (a, b, c, d, e),
            (0xDEAD_BEEF, -5, true, [1, 2, 3], vec![7; 4])
        );
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn truncated_input_is_an_error() {
        let mut v = 0u32;
        assert_eq!(v.load(&mut Reader::new(&[1, 2])), Err(Error::UnexpectedEof));
    }

    #[test]
    fn vec_length_mismatch_is_an_error() {
        let mut w = Writer::new();
        vec![0u8; 3].save(&mut w);
        let bytes = w.into_bytes();
        let mut v = vec![0u8; 4];
        assert_eq!(
            v.load(&mut Reader::new(&bytes)),
            Err(Error::Invalid("buffer length"))
        );
    }
}
