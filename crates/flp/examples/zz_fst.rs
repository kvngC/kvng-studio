//! Prints the plugin-data block of each .fst preset as u32 columns.
use flp::event::{self, Events};
fn main() {
    for path in std::env::args().skip(1) {
        let file = std::fs::read(&path).unwrap();
        let (_, data, base) = event::split(&file).unwrap();
        for ev in Events::new(data, base) {
            let ev = ev.unwrap();
            if ev.id == 213 {
                let b = ev.as_bytes();
                let vals: Vec<String> = b.chunks(4).map(|c| if c.len()==4 { u32::from_le_bytes(c.try_into().unwrap()).to_string() } else { format!("{:?}", c) }).collect();
                let name = std::path::Path::new(&path).file_stem().unwrap().to_string_lossy().to_string();
                println!("{:34} {}", name, vals.join(","));
            }
        }
    }
}
