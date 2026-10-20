//! Control messages between KVNG Studio and a plugin helper process.
//!
//! Each message is framed as `u32 length` + body. Bodies start with a tag byte;
//! strings and byte blobs are `u32 length` + bytes.

use std::io::{self, Read, Write};

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    LoadVst3 { path: String, name: Option<String>, sample_rate: f64, max_block: u32 },
    LoadAu { kind: String, subtype: String, manufacturer: String, sample_rate: f64, max_block: u32 },
    RestoreVst3 { component: Vec<u8>, controller: Option<Vec<u8>> },
    RestoreAu { class_info: Vec<u8> },
    SaveState,
    LoadState(Vec<u8>),
    OpenEditor,
    CloseEditor,
    Shutdown,
    ListParams,
    /// Parameters the user moved in the plugin window since the last request.
    TakeParamChanges,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    Loaded { name: String, inputs: u32, outputs: u32, latency: u32, accepts_midi: bool },
    Ok,
    State(Vec<u8>),
    Error(String),
    /// Parameter names and normalised values.
    Params(Vec<(String, f32)>),
    /// (parameter index, normalised value) changes.
    ParamChanges(Vec<(u32, f32)>),
}

pub fn send<W: Write, M: Encode>(w: &mut W, msg: &M) -> io::Result<()> {
    let mut body = Vec::new();
    msg.encode(&mut body);
    w.write_all(&(body.len() as u32).to_le_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

pub fn recv<R: Read, M: Encode>(r: &mut R) -> io::Result<M> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > 512 * 1024 * 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    M::decode(&mut Cursor { buf: &body, pos: 0 }).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed message"))
}

pub trait Encode: Sized {
    fn encode(&self, out: &mut Vec<u8>);
    fn decode(c: &mut Cursor) -> Option<Self>;
}

pub struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.buf.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn f64(&mut self) -> Option<f64> {
        self.take(8).map(|b| f64::from_le_bytes(b.try_into().unwrap()))
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.u32()? as usize;
        self.take(n).map(<[u8]>::to_vec)
    }
    fn string(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?).ok()
    }
    fn opt_bytes(&mut self) -> Option<Option<Vec<u8>>> {
        match self.u8()? {
            0 => Some(None),
            _ => Some(Some(self.bytes()?)),
        }
    }
    fn opt_string(&mut self) -> Option<Option<String>> {
        match self.u8()? {
            0 => Some(None),
            _ => Some(Some(self.string()?)),
        }
    }
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b);
}

fn put_opt(out: &mut Vec<u8>, b: Option<&[u8]>) {
    match b {
        Some(b) => {
            out.push(1);
            put_bytes(out, b);
        }
        None => out.push(0),
    }
}

impl Encode for Request {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Request::LoadVst3 { path, name, sample_rate, max_block } => {
                out.push(1);
                put_bytes(out, path.as_bytes());
                put_opt(out, name.as_deref().map(str::as_bytes));
                out.extend_from_slice(&sample_rate.to_le_bytes());
                out.extend_from_slice(&max_block.to_le_bytes());
            }
            Request::LoadAu { kind, subtype, manufacturer, sample_rate, max_block } => {
                out.push(2);
                for s in [kind, subtype, manufacturer] {
                    put_bytes(out, s.as_bytes());
                }
                out.extend_from_slice(&sample_rate.to_le_bytes());
                out.extend_from_slice(&max_block.to_le_bytes());
            }
            Request::RestoreVst3 { component, controller } => {
                out.push(3);
                put_bytes(out, component);
                put_opt(out, controller.as_deref());
            }
            Request::RestoreAu { class_info } => {
                out.push(4);
                put_bytes(out, class_info);
            }
            Request::SaveState => out.push(5),
            Request::LoadState(b) => {
                out.push(6);
                put_bytes(out, b);
            }
            Request::OpenEditor => out.push(7),
            Request::CloseEditor => out.push(8),
            Request::Shutdown => out.push(9),
            Request::ListParams => out.push(10),
            Request::TakeParamChanges => out.push(11),
        }
    }

    fn decode(c: &mut Cursor) -> Option<Self> {
        Some(match c.u8()? {
            1 => Request::LoadVst3 { path: c.string()?, name: c.opt_string()?, sample_rate: c.f64()?, max_block: c.u32()? },
            2 => Request::LoadAu {
                kind: c.string()?,
                subtype: c.string()?,
                manufacturer: c.string()?,
                sample_rate: c.f64()?,
                max_block: c.u32()?,
            },
            3 => Request::RestoreVst3 { component: c.bytes()?, controller: c.opt_bytes()? },
            4 => Request::RestoreAu { class_info: c.bytes()? },
            5 => Request::SaveState,
            6 => Request::LoadState(c.bytes()?),
            7 => Request::OpenEditor,
            8 => Request::CloseEditor,
            9 => Request::Shutdown,
            10 => Request::ListParams,
            11 => Request::TakeParamChanges,
            _ => return None,
        })
    }
}

impl Encode for Response {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Response::Loaded { name, inputs, outputs, latency, accepts_midi } => {
                out.push(1);
                put_bytes(out, name.as_bytes());
                for v in [inputs, outputs, latency] {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                out.push(*accepts_midi as u8);
            }
            Response::Ok => out.push(2),
            Response::State(b) => {
                out.push(3);
                put_bytes(out, b);
            }
            Response::Error(e) => {
                out.push(4);
                put_bytes(out, e.as_bytes());
            }
            Response::ParamChanges(list) => {
                out.push(6);
                out.extend_from_slice(&(list.len() as u32).to_le_bytes());
                for (index, value) in list {
                    out.extend_from_slice(&index.to_le_bytes());
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
            Response::Params(list) => {
                out.push(5);
                out.extend_from_slice(&(list.len() as u32).to_le_bytes());
                for (name, value) in list {
                    put_bytes(out, name.as_bytes());
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
    }

    fn decode(c: &mut Cursor) -> Option<Self> {
        Some(match c.u8()? {
            1 => Response::Loaded {
                name: c.string()?,
                inputs: c.u32()?,
                outputs: c.u32()?,
                latency: c.u32()?,
                accepts_midi: c.u8()? != 0,
            },
            2 => Response::Ok,
            3 => Response::State(c.bytes()?),
            4 => Response::Error(c.string()?),
            5 => {
                let n = c.u32()? as usize;
                let mut list = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    let name = c.string()?;
                    let value = f32::from_bits(c.u32()?);
                    list.push((name, value));
                }
                Response::Params(list)
            }
            6 => {
                let n = c.u32()? as usize;
                let mut list = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    list.push((c.u32()?, f32::from_bits(c.u32()?)));
                }
                Response::ParamChanges(list)
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let requests = vec![
            Request::LoadVst3 { path: "/a.vst3".into(), name: Some("A".into()), sample_rate: 48000.0, max_block: 512 },
            Request::LoadAu { kind: "aumu".into(), subtype: "k105".into(), manufacturer: "Aufb".into(), sample_rate: 44100.0, max_block: 256 },
            Request::RestoreVst3 { component: vec![1, 2, 3], controller: None },
            Request::RestoreAu { class_info: vec![9; 1000] },
            Request::Shutdown,
            Request::TakeParamChanges,
        ];
        for req in requests {
            let mut buf = Vec::new();
            send(&mut buf, &req).unwrap();
            assert_eq!(recv::<_, Request>(&mut buf.as_slice()).unwrap(), req);
        }
        for resp in [Response::Loaded { name: "Kia".into(), inputs: 2, outputs: 2, latency: 128, accepts_midi: true }, Response::ParamChanges(vec![(3, 0.25), (2574, 1.0)])] {
            let mut buf = Vec::new();
            send(&mut buf, &resp).unwrap();
            assert_eq!(recv::<_, Response>(&mut buf.as_slice()).unwrap(), resp);
        }
    }
}
