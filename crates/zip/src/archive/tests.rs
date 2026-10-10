//! Archives built by hand with [`crate::test_util::zip`].

use super::*;
use crate::test_util::{File, zip};

fn rom(len: usize) -> Vec<u8> {
    (0..len).map(|i| ((i * 7) ^ (i >> 8)) as u8).collect()
}

#[test]
fn reads_stored_and_deflated_files() {
    let a = rom(1000);
    let b = b"hello".repeat(100);
    let bytes = zip(
        &[File::new("a.bin", &a, 0), File::new("dir/b.txt", &b, 8)],
        b"",
    );
    let archive = Archive::parse(&bytes).unwrap();
    let names: Vec<_> = archive.entries().iter().map(Entry::name).collect();
    assert_eq!(names, ["a.bin", "dir/b.txt"]);
    let e = &archive.entries()[1];
    assert_eq!(e.method(), Method::Deflated);
    assert_eq!(e.size(), 500);
    assert_eq!(e.crc32(), crc32(&b));
    assert_eq!(archive.read(&archive.entries()[0]).unwrap(), a);
    assert_eq!(archive.read(e).unwrap(), b);
}

#[test]
fn comment_and_prepended_data() {
    let a = rom(300);
    let mut bytes = b"#!/bin/sh\necho self-extracting stub\n".to_vec();
    let stub = bytes.len();
    // A comment that contains a fake EOCD signature must not confuse us.
    bytes.extend(zip(&[File::new("a.md", &a, 8)], b"PK\x05\x06 not a record"));
    let archive = Archive::parse(&bytes).unwrap();
    assert_eq!(archive.comment(), b"PK\x05\x06 not a record");
    assert_eq!(archive.entries()[0].local_header, stub);
    assert_eq!(archive.read(&archive.entries()[0]).unwrap(), a);
}

#[test]
fn empty_archive() {
    let bytes = zip(&[], b"");
    assert_eq!(bytes.len(), 22);
    let archive = Archive::parse(&bytes).unwrap();
    assert!(archive.entries().is_empty());
    assert!(find_rom(&archive).is_none());
}

#[test]
fn not_a_zip() {
    assert_eq!(Archive::parse(b"").unwrap_err(), Error::NotAZip);
    assert_eq!(Archive::parse(&rom(4096)).unwrap_err(), Error::NotAZip);
    // Starts like an archive but has no end: cut short.
    let bytes = zip(&[File::new("a.bin", &rom(100), 0)], b"");
    assert_eq!(
        Archive::parse(&bytes[..120]).unwrap_err(),
        Error::Truncated("no end of central directory")
    );
}

#[test]
fn crc_mismatch() {
    let a = rom(100);
    let mut bytes = zip(&[File::new("a.bin", &a, 0)], b"");
    bytes[30 + 5 + 10] ^= 1; // a byte of the file data
    let archive = Archive::parse(&bytes).unwrap();
    assert_eq!(
        archive.read(&archive.entries()[0]),
        Err(Error::CrcMismatch {
            expected: crc32(&a),
            actual: {
                let mut damaged = a.clone();
                damaged[10] ^= 1;
                crc32(&damaged)
            }
        })
    );
}

#[test]
fn wrong_size() {
    let a = rom(100);
    let mut bytes = zip(&[File::new("a.bin", &a, 8)], b"");
    // Central directory record starts after the local header and data;
    // shrink its uncompressed size (offset 24).
    let cd = bytes.len() - 22 - 46 - 5;
    bytes[cd + 24] = 99;
    let archive = Archive::parse(&bytes).unwrap();
    assert_eq!(
        archive.read(&archive.entries()[0]),
        Err(Error::WrongSize { expected: 99 })
    );
    bytes[cd + 24] = 101;
    let archive = Archive::parse(&bytes).unwrap();
    assert_eq!(
        archive.read(&archive.entries()[0]),
        Err(Error::WrongSize { expected: 101 })
    );
}

#[test]
fn truncated_archives_are_errors() {
    let a = rom(2000);
    let bytes = zip(
        &[File::new("a.bin", &a, 8), File::new("b.md", &a, 0)],
        b"comment",
    );
    for cut in 0..bytes.len() {
        let result = Archive::parse(&bytes[..cut]).and_then(|archive| {
            let entry = archive.entries().first().ok_or(Error::NotAZip)?.clone();
            archive.read(&entry)
        });
        assert!(result.is_err(), "cut at {cut}");
    }
}

#[test]
fn unsupported_features() {
    let a = rom(10);
    let mut encrypted = File::new("a.bin", &a, 0);
    encrypted.flags = FLAG_ENCRYPTED;
    let bytes = zip(&[encrypted, File::new("b.bin", &a, 12)], b"");
    let archive = Archive::parse(&bytes).unwrap();
    assert!(archive.entries()[0].is_encrypted());
    assert_eq!(archive.read(&archive.entries()[0]), Err(Error::Encrypted));
    assert_eq!(archive.entries()[1].method(), Method::Other(12));
    assert_eq!(
        archive.read(&archive.entries()[1]),
        Err(Error::UnsupportedMethod(12))
    );

    // ZIP64: sizes of all ones in the central directory.
    let mut bytes = zip(&[File::new("a.bin", &a, 0)], b"");
    let cd = bytes.len() - 22 - 46 - 5;
    bytes[cd + 20..cd + 28].fill(0xFF);
    assert_eq!(Archive::parse(&bytes).unwrap_err(), Error::Zip64);

    // Split archives.
    let mut bytes = zip(&[File::new("a.bin", &a, 0)], b"");
    let eocd = bytes.len() - 22;
    bytes[eocd + 4] = 1;
    assert_eq!(Archive::parse(&bytes).unwrap_err(), Error::MultiDisk);
}

#[test]
fn bad_signatures() {
    let a = rom(10);
    let bytes = zip(&[File::new("a.bin", &a, 0)], b"");
    let mut damaged = bytes.clone();
    damaged[0] = b'X';
    let archive = Archive::parse(&damaged).unwrap();
    assert_eq!(
        archive.read(&archive.entries()[0]),
        Err(Error::BadSignature("local header"))
    );
    let mut damaged = bytes;
    let cd = damaged.len() - 22 - 46 - 5;
    damaged[cd] = b'X';
    assert_eq!(
        Archive::parse(&damaged).unwrap_err(),
        Error::BadSignature("central directory")
    );
}

#[test]
fn too_large_files_are_refused_before_reading() {
    let a = rom(10);
    let mut bytes = zip(&[File::new("a.bin", &a, 8)], b"");
    let cd = bytes.len() - 22 - 46 - 5;
    bytes[cd + 24..cd + 28].copy_from_slice(&(65u32 << 20).to_le_bytes());
    let archive = Archive::parse(&bytes).unwrap();
    assert_eq!(
        archive.read(&archive.entries()[0]),
        Err(Error::TooLarge(65 << 20))
    );
}

#[test]
fn rom_selection() {
    let small = rom(100);
    let big = rom(5000);
    let bytes = zip(
        &[
            File::new("readme.txt", &big, 0),
            File::new("game.bin", &small, 0),
            File::new("Game (Europe).GEN", &big, 8),
            File::new("Game (USA).Md", &big, 8),
            File::new("roms.bin/", b"", 0),
            File::new("__MACOSX/huge.bin", &rom(9000), 0),
            File::new("._huge.md", &rom(9000), 0),
        ],
        b"",
    );
    let archive = Archive::parse(&bytes).unwrap();
    // Largest wins; the first of two equally large files.
    assert_eq!(find_rom(&archive).unwrap().name(), "Game (Europe).GEN");

    for name in [
        "a.md", "a.MD", "x/a.bin", "a.gen", "a.smd", "a.68k", "a.SGD",
    ] {
        assert!(is_rom_name(name), "{name}");
    }
    for name in [
        "a.txt",
        "md",
        ".md",
        "a.md.txt",
        "__MACOSX/a.md",
        "x/._a.bin",
    ] {
        assert!(!is_rom_name(name), "{name}");
    }
}

#[test]
fn names() {
    assert_eq!(CP437_HIGH.chars().count(), 128);
    assert_eq!(decode_name(b"caf\x82.bin", false), "café.bin");
    assert_eq!(decode_name("café.bin".as_bytes(), true), "café.bin");
    assert_eq!(decode_name(b"plain.md", false), "plain.md");
}

#[test]
fn random_damage_never_panics() {
    let a = rom(3000);
    let b = b"some text ".repeat(50);
    let bytes = zip(
        &[File::new("a.bin", &a, 8), File::new("b.txt", &b, 0)],
        b"c",
    );
    let mut seed = 0x0BAD_CAFEu32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as usize
    };
    for _ in 0..5000 {
        let mut damaged = bytes.clone();
        for _ in 0..1 + next() % 4 {
            let i = next() % damaged.len();
            damaged[i] = next() as u8;
        }
        if let Ok(archive) = Archive::parse(&damaged) {
            for entry in archive.entries() {
                let _ = archive.read(entry);
            }
            let _ = find_rom(&archive);
        }
    }
}
