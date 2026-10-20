//! Prints every raw event in an FLP file: `cargo run -p flp --example dump_events -- file.flp`

use flp::event::{self, Events, Payload};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: dump_events <file.flp>")?;
    let file = std::fs::read(path)?;
    let (header, data, base) = event::split(&file)?;
    println!("{header:?}");
    for (i, ev) in Events::new(data, base).enumerate() {
        let ev = ev?;
        let shown = match ev.payload {
            Payload::U8(v) => format!("u8  {v}"),
            Payload::U16(v) => format!("u16 {v}"),
            Payload::U32(v) => format!("u32 {v}"),
            Payload::Bytes(b) => {
                let preview: Vec<String> = b.iter().take(24).map(|x| format!("{x:02x}")).collect();
                format!("[{}] {}", b.len(), preview.join(" "))
            }
        };
        println!("{i:6} @{:8} id={:3} {shown}", ev.offset, ev.id);
    }
    Ok(())
}
