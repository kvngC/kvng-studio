fn main() {
    let a: Vec<String> = std::env::args().collect();
    let p = flp::read(&a[1]).unwrap();
    let pls = p.channels.iter().filter_map(|c| c.plugin.as_ref()).chain(p.inserts.iter().flat_map(|i| i.slots.iter().map(|s| &s.plugin)));
    let mut seen = std::collections::HashSet::new();
    for pl in pls.filter(|p| !p.is_third_party() && !p.internal_name.is_empty()) {
        if !seen.insert(pl.internal_name.clone()) { continue }
        println!("== {} ({} bytes)", pl.internal_name, pl.data.len());
        for (i, row) in pl.data.chunks(16).enumerate().take(if pl.data.len() > 400 { 12 } else { 20 }) {
            let hex: Vec<String> = row.iter().map(|b| format!("{b:02x}")).collect();
            let txt: String = row.iter().map(|&b| if (32..127).contains(&b) { b as char } else { '.' }).collect();
            println!("{:4} {:48} {txt}", i * 16, hex.join(" "));
        }
        // printable runs (utf16 or ascii)
        let mut s = String::new();
        let mut runs = vec![];
        for w in pl.data.chunks(2) { if w.len()==2 && w[1]==0 && (32..127).contains(&w[0]) { s.push(w[0] as char) } else { if s.len() > 5 { runs.push(s.clone()) } s.clear() } }
        println!("utf16 strings: {:?}", runs);
    }
}
