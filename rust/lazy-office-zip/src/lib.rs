//! A minimal zip writer for Office packages and page archives: every entry stored uncompressed (Word and LibreOffice
//! read stored entries), dated 1980-01-01, in the order given, with a CRC-32 per entry. The same bytes as Lazy Scan's
//! Swift and C# writers for the same entries. No zip64: a package stays under 4 GB and 65,535 entries.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use std::io::{self, Write};

/// One file in a package: its path inside it ("word/document.xml") and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
}

impl Entry {
    pub fn new(name: impl Into<String>, data: impl Into<Vec<u8>>) -> Entry {
        Entry {
            name: name.into(),
            data: data.into(),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    TooManyEntries,
    TooLarge,
    Io(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::TooManyEntries => f.write_str("Too many entries for a zip without zip64."),
            Error::TooLarge => f.write_str("The package is too large for a zip without zip64."),
            Error::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

/// 1.0: stored entries need nothing newer.
const VERSION_NEEDED: u16 = 10;
/// 2.0, MS-DOS attributes.
const VERSION_MADE_BY: u16 = 20;
const DOS_TIME: u16 = 0;
/// 1980-01-01, the earliest date a zip can hold: the same bytes on every save.
const DOS_DATE: u16 = 1 << 5 | 1;
const LOCAL_HEADER: u64 = 30;
const CENTRAL_RECORD: u64 = 46;
const END_RECORD: u64 = 22;

const fn table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB88320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

static TABLE: [u32; 256] = table();

/// The CRC-32 of zip and PNG (polynomial 0xEDB88320, reflected; "123456789" gives 0xCBF43926).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = TABLE[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// The size in bytes of the package `make` writes for `entries`, worked out without writing it.
pub fn package_size(entries: &[Entry]) -> u64 {
    entries.iter().fold(END_RECORD, |size, e| {
        let name = e.name.len() as u64;
        size + LOCAL_HEADER + name + e.data.len() as u64 + CENTRAL_RECORD + name
    })
}

/// The zip package holding `entries`, in that order.
pub fn make(entries: &[Entry]) -> Result<Vec<u8>, Error> {
    let mut writer = Writer::new(Vec::new());
    for entry in entries {
        writer.add(&entry.name, &entry.data)?;
    }
    writer.finish()
}

/// Writes a package one entry at a time: the same bytes `make` returns for the same entries, without a second copy of
/// the whole package in memory.
pub struct Writer<W: Write> {
    output: W,
    central: Vec<u8>,
    offset: u64,
    count: usize,
}

impl<W: Write> Writer<W> {
    pub fn new(output: W) -> Writer<W> {
        Writer {
            output,
            central: Vec::new(),
            offset: 0,
            count: 0,
        }
    }

    pub fn add(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
        if self.count == u16::MAX as usize {
            return Err(Error::TooManyEntries);
        }
        if self.offset > u32::MAX as u64 || data.len() as u64 > u32::MAX as u64 {
            return Err(Error::TooLarge);
        }
        let flags: u16 = if name.is_ascii() { 0 } else { 0x0800 };
        let crc = crc32(data);
        let size = data.len() as u32;
        let length = name.len() as u16;

        let mut local = Vec::with_capacity(LOCAL_HEADER as usize + name.len());
        local.extend(0x04034B50u32.to_le_bytes());
        local.extend(VERSION_NEEDED.to_le_bytes());
        local.extend(flags.to_le_bytes());
        local.extend(0u16.to_le_bytes()); // stored
        local.extend(DOS_TIME.to_le_bytes());
        local.extend(DOS_DATE.to_le_bytes());
        local.extend(crc.to_le_bytes());
        local.extend(size.to_le_bytes());
        local.extend(size.to_le_bytes());
        local.extend(length.to_le_bytes());
        local.extend(0u16.to_le_bytes()); // no extra field
        local.extend(name.as_bytes());
        self.output.write_all(&local)?;
        self.output.write_all(data)?;

        let c = &mut self.central;
        c.extend(0x02014B50u32.to_le_bytes());
        c.extend(VERSION_MADE_BY.to_le_bytes());
        c.extend(VERSION_NEEDED.to_le_bytes());
        c.extend(flags.to_le_bytes());
        c.extend(0u16.to_le_bytes());
        c.extend(DOS_TIME.to_le_bytes());
        c.extend(DOS_DATE.to_le_bytes());
        c.extend(crc.to_le_bytes());
        c.extend(size.to_le_bytes());
        c.extend(size.to_le_bytes());
        c.extend(length.to_le_bytes());
        c.extend([0; 2]); // extra field
        c.extend([0; 2]); // comment
        c.extend([0; 2]); // disk number
        c.extend([0; 2]); // internal attributes
        c.extend([0; 4]); // external attributes
        c.extend((self.offset as u32).to_le_bytes());
        c.extend(name.as_bytes());

        self.offset += LOCAL_HEADER + name.len() as u64 + data.len() as u64;
        self.count += 1;
        Ok(())
    }

    /// Writes the central directory and the end record and hands the output back. Call once, after the last entry.
    pub fn finish(mut self) -> Result<W, Error> {
        if self.offset + self.central.len() as u64 > u32::MAX as u64 {
            return Err(Error::TooLarge);
        }
        self.output.write_all(&self.central)?;
        let mut end = Vec::with_capacity(END_RECORD as usize);
        end.extend(0x06054B50u32.to_le_bytes());
        end.extend([0; 2]); // this disk
        end.extend([0; 2]); // the disk the directory starts on
        end.extend((self.count as u16).to_le_bytes());
        end.extend((self.count as u16).to_le_bytes());
        end.extend((self.central.len() as u32).to_le_bytes());
        end.extend((self.offset as u32).to_le_bytes());
        end.extend([0; 2]); // no comment
        self.output.write_all(&end)?;
        Ok(self.output)
    }
}
