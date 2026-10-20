//! Low-level FLP container: header + a flat stream of tagged events.
//!
//! Event IDs encode their payload size:
//! - `0..=63`    1-byte value
//! - `64..=127`  2-byte value
//! - `128..=191` 4-byte value
//! - `192..=255` variable-length payload, prefixed by a 7-bit varint length

use crate::Error;

pub const BYTE: u8 = 0;
pub const WORD: u8 = 64;
pub const DWORD: u8 = 128;
pub const TEXT: u8 = 192;
pub const DATA: u8 = 208;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payload<'a> {
    U8(u8),
    U16(u16),
    U32(u32),
    Bytes(&'a [u8]),
}

#[derive(Debug, Clone, Copy)]
pub struct Event<'a> {
    pub id: u8,
    /// Byte offset of the event's ID within the file (for diagnostics).
    pub offset: usize,
    pub payload: Payload<'a>,
}

impl<'a> Event<'a> {
    pub fn as_u32(&self) -> Option<u32> {
        match self.payload {
            Payload::U8(v) => Some(v as u32),
            Payload::U16(v) => Some(v as u32),
            Payload::U32(v) => Some(v),
            Payload::Bytes(_) => None,
        }
    }

    pub fn as_bytes(&self) -> &'a [u8] {
        match self.payload {
            Payload::Bytes(b) => b,
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub format: i16,
    pub channel_count: u16,
    pub ppq: u16,
}

/// Parses the `FLhd` header and returns it with the raw `FLdt` event bytes.
pub fn split(file: &[u8]) -> Result<(Header, &[u8], usize), Error> {
    let mut r = Reader::new(file, 0);
    if r.take(4)? != b"FLhd" {
        return Err(Error::BadMagic);
    }
    let header_len = r.u32()? as usize;
    let header_bytes = r.take(header_len)?;
    if header_bytes.len() < 6 {
        return Err(Error::Truncated(r.pos));
    }
    let header = Header {
        format: i16::from_le_bytes([header_bytes[0], header_bytes[1]]),
        channel_count: u16::from_le_bytes([header_bytes[2], header_bytes[3]]),
        ppq: u16::from_le_bytes([header_bytes[4], header_bytes[5]]),
    };
    if r.take(4)? != b"FLdt" {
        return Err(Error::BadMagic);
    }
    let data_len = r.u32()? as usize;
    let start = r.pos;
    // Some writers store a wrong data length; trust the file size instead.
    let end = (start + data_len).min(file.len());
    Ok((header, &file[start..end], start))
}

/// Iterates over the events in an `FLdt` chunk.
pub struct Events<'a> {
    r: Reader<'a>,
}

impl<'a> Events<'a> {
    pub fn new(data: &'a [u8], base_offset: usize) -> Self {
        Self { r: Reader::new(data, base_offset) }
    }
}

impl<'a> Iterator for Events<'a> {
    type Item = Result<Event<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.r.remaining() == 0 {
            return None;
        }
        let offset = self.r.abs_pos();
        let result = (|| {
            let id = self.r.u8()?;
            let payload = match id {
                0..=63 => Payload::U8(self.r.u8()?),
                64..=127 => Payload::U16(self.r.u16()?),
                128..=191 => Payload::U32(self.r.u32()?),
                _ => {
                    let len = self.r.varint()?;
                    Payload::Bytes(self.r.take(len)?)
                }
            };
            Ok(Event { id, offset, payload })
        })();
        if result.is_err() {
            // Stop after the first malformed event instead of looping on it.
            self.r.pos = self.r.buf.len();
        }
        Some(result)
    }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    base: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8], base: usize) -> Self {
        Self { buf, pos: 0, base }
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn abs_pos(&self) -> usize {
        self.base + self.pos
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.remaining() < n {
            return Err(Error::Truncated(self.abs_pos()));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, Error> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, Error> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64, Error> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes(b.try_into().unwrap()))
    }

    fn varint(&mut self) -> Result<usize, Error> {
        let mut value: usize = 0;
        for shift in (0..35).step_by(7) {
            let b = self.u8()?;
            value |= ((b & 0x7f) as usize) << shift;
            if b & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Error::BadVarint(self.abs_pos()))
    }
}
