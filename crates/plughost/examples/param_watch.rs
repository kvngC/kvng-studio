//! Loads an Audio Unit and checks parameter-change reporting:
//! `cargo run -p plughost --example param_watch -- aumu k105 Aufb`
use plughost::Host;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [kind, subtype, manufacturer] = &args[..] else {
        eprintln!("usage: param_watch <type> <subtype> <manufacturer>");
        std::process::exit(2);
    };
    let host = Host::init();
    let mut plugin = host.load_au(kind, subtype, manufacturer, 48_000.0, 512).expect("load");
    let params = plugin.parameters();
    println!("{}: {} parameters", plugin.name(), params.len());
    let _ = plugin.take_param_changes(); // anything from loading
    // KVNG's own changes (automation playback) must not come back.
    for i in 0..params.len().min(8) as u32 {
        plugin.set_param(i, 0.37);
    }
    let echoed = plugin.take_param_changes();
    println!("host-set changes reported back: {} (expected 0)", echoed.len());
    assert!(echoed.is_empty());
    println!("ok");
}
