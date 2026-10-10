//! Data written by an independent implementation (Python's zlib and
//! zipfile, see `fixtures/make_fixtures.py`) must decode exactly.

use std::path::{Path, PathBuf};

use gase_zip::crc32::crc32;
use gase_zip::{Archive, Error, Method, find_rom, inflate, inflate_reference};

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The fake ROM of `make_fixtures.py`.
fn rom(size: usize) -> Vec<u8> {
    (0..size).map(|i| ((i * 7) ^ (i >> 8)) as u8).collect()
}

#[test]
fn zlib_streams_at_every_level_and_strategy() {
    let manifest = String::from_utf8(fixture("deflate.txt")).unwrap();
    let mut count = 0;
    for line in manifest.lines().filter(|l| !l.starts_with('#')) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [name, size, crc] = fields[..] else {
            panic!("bad manifest line {line:?}");
        };
        let size: usize = size.parse().unwrap();
        let crc = u32::from_str_radix(crc, 16).unwrap();
        let packed = fixture(name);

        let data = inflate(&packed, 1 << 20).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(data.len(), size, "{name}");
        assert_eq!(crc32(&data), crc, "{name}");
        assert_eq!(inflate_reference(&packed, 1 << 20).unwrap(), data, "{name}");

        // The exact size is enough room; one byte less is not.
        assert_eq!(inflate(&packed, size).unwrap().len(), size);
        if size > 0 {
            assert!(inflate(&packed, size - 1).is_err(), "{name}");
        }
        // Every truncation is an error, never a panic.
        for cut in (0..packed.len()).step_by(packed.len() / 50 + 1) {
            assert!(
                inflate(&packed[..cut], 1 << 20).is_err(),
                "{name} cut {cut}"
            );
        }
        count += 1;
    }
    assert_eq!(count, 19);
}

#[test]
fn several_files() {
    let bytes = fixture("several.zip");
    let archive = Archive::parse(&bytes).unwrap();
    assert_eq!(archive.comment(), b"gase-zip test archive");
    let names: Vec<&str> = archive.entries().iter().map(|e| e.name()).collect();
    assert_eq!(
        names,
        [
            "readme.txt",
            "Game (USA)/",
            "Game (USA)/Game (USA).MD",
            "Game (USA)/small.bin",
            "__MACOSX/Game (USA)/._Game (USA).MD",
            "Game (USA)/._hidden.bin",
            "Game (USA)/screenshot.png",
        ]
    );
    // Every file extracts, with a correct CRC.
    for entry in archive.entries() {
        let data = archive.read(entry).unwrap();
        assert_eq!(data.len() as u64, entry.size(), "{}", entry.name());
    }
    assert!(archive.entries()[1].is_dir());
    assert_eq!(archive.entries()[3].method(), Method::Stored);

    let found = find_rom(&archive).unwrap();
    assert_eq!(found.name(), "Game (USA)/Game (USA).MD");
    assert_eq!(found.method(), Method::Deflated);
    assert_eq!(archive.read(found).unwrap(), rom(3000));
}

#[test]
fn no_rom_and_empty() {
    let bytes = fixture("no_rom.zip");
    let archive = Archive::parse(&bytes).unwrap();
    assert_eq!(archive.entries().len(), 2);
    assert!(find_rom(&archive).is_none());

    let bytes = fixture("empty.zip");
    let archive = Archive::parse(&bytes).unwrap();
    assert!(archive.entries().is_empty());
    assert!(find_rom(&archive).is_none());
}

#[test]
fn streamed_archive_with_data_descriptor() {
    let bytes = fixture("streamed.zip");
    let archive = Archive::parse(&bytes).unwrap();
    let rom_entry = find_rom(&archive).unwrap();
    assert_eq!(rom_entry.name(), "game.gen");
    assert_eq!(archive.read(rom_entry).unwrap(), rom(5000));
}

#[test]
fn damaged_fixture_is_detected() {
    let mut bytes = fixture("streamed.zip");
    // Flip a bit in the middle of the compressed data: either the stream
    // breaks or the CRC catches it.
    bytes[1000] ^= 0x10;
    let archive = Archive::parse(&bytes).unwrap();
    let entry = find_rom(&archive).unwrap();
    let err = archive.read(entry).unwrap_err();
    assert!(
        matches!(
            err,
            Error::Inflate(_) | Error::CrcMismatch { .. } | Error::WrongSize { .. }
        ),
        "{err:?}"
    );
}

#[test]
fn random_bytes_never_panic() {
    let mut seed = 0x2545_F491u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let several = fixture("several.zip");
    for round in 0..3000 {
        let len = (next() % 512) as usize;
        let mut data: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        match round % 3 {
            // Pure noise.
            0 => {}
            // Noise ending in a plausible EOCD with random fields.
            1 => {
                data.extend_from_slice(b"PK\x05\x06\0\0\0\0");
                data.extend((0..14).map(|_| next() as u8));
                let n = data.len();
                data[n - 2..].fill(0);
            }
            // A real archive with random bytes overwritten.
            _ => {
                data = several.clone();
                for _ in 0..1 + next() % 8 {
                    let i = next() as usize % data.len();
                    data[i] = next() as u8;
                }
            }
        }
        let _ = inflate(&data, 1 << 16);
        if let Ok(archive) = Archive::parse(&data) {
            for entry in archive.entries() {
                let _ = archive.read(entry);
            }
            let _ = find_rom(&archive);
        }
    }
}

/// Zipped copies of real ROMs must extract byte for byte. Put `game.bin`
/// and `game.bin.zip` (made by any zip tool) side by side in a directory
/// and run with `GASE_ZIP_ROMS=<directory>` and `--ignored`.
#[test]
#[ignore = "needs zipped test ROMs, see GASE_ZIP_ROMS"]
fn zipped_test_roms() {
    let dir = PathBuf::from(std::env::var("GASE_ZIP_ROMS").expect("GASE_ZIP_ROMS"));
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "zip") {
            continue;
        }
        let original = std::fs::read(path.with_extension("")).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let archive = Archive::parse(&bytes).unwrap();
        let rom = find_rom(&archive).expect("a ROM in the archive");
        assert_eq!(archive.read(rom).unwrap(), original, "{}", path.display());
        checked += 1;
    }
    assert!(checked > 0, "no .zip files in {}", dir.display());
}
