//! KVNG Studio entry point: the app, plus command-line tools.

mod bench;
mod gui;
mod play;
mod render;

use std::process::ExitCode;

const USAGE: &str = "\
usage:
  kvng-studio [project.flp|.kvng]    open the app (optionally with a project)
  kvng-studio convert <project> --out file.kvng
  kvng-studio scan [--full]           find installed plugins (and capture their pictures)
  kvng-studio play   <project.flp> [--from <bar>] [--for <seconds>]
  kvng-studio bounce <project.flp> [--from <bar>] [--out song.wav]
  kvng-studio info   <project.flp>
  kvng-studio probe  <project.flp> --channel <name>
  kvng-studio render <project.flp> --channel <name> [--pattern <name>] [--out file.wav] [--editor] [--no-state]
  kvng-studio bench [--quick]         engine/mixer/channel-strip CPU benchmark";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("info") => args.get(1).ok_or_else(|| USAGE.to_string()).and_then(|p| info(p)),
        Some("render") => render::run(&args[1..]),
        Some("play") => play::play(&args[1..]),
        Some("bounce") => play::bounce(&args[1..]),
        Some("probe") => render::probe(&args[1..]),
        Some("bench") => bench::run(&args[1..]),
        Some("convert") => play::convert(&args[1..]),
        Some("scan") => scan(args.iter().any(|a| a == "--full")),
        None => gui::run(None),
        Some(path) if path.ends_with(".flp") || path.ends_with(".kvng") => gui::run(Some(std::path::PathBuf::from(path))),
        Some("open") => gui::run(args.get(1).map(std::path::PathBuf::from)),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Where KVNG Studio keeps its plugin database and snapshots.
pub fn data_dir() -> std::path::PathBuf {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Library/Application Support/KVNG Studio")
}

fn scan(full: bool) -> Result<(), String> {
    let scanner = bridge::scan::Scanner::new(data_dir());
    let (plugins, failures) = scanner.scan(full, &mut |done, total, name| {
        print!("\r  [{done}/{total}] {name:<50}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    })?;
    println!();
    for p in &plugins {
        println!(
            "  {:<4} {:<11} {:<28} {:<16} {:<6} {}",
            p.format_name(),
            if p.is_instrument { "instrument" } else { "effect" },
            p.name,
            p.vendor,
            p.arch,
            if p.snapshot.is_some() { "picture" } else { "-" }
        );
    }
    println!("{} plugins, {} with pictures, {} failed", plugins.len(), plugins.iter().filter(|p| p.snapshot.is_some()).count(), failures.len());
    for f in failures {
        println!("  ! {f}");
    }
    Ok(())
}

fn info(path: &str) -> Result<(), String> {
    let p = flp::read(path).map_err(|e| format!("{path}: {e}"))?;
    println!("FL {} | {} BPM | {} channels | {} patterns", p.version, p.tempo, p.channels.len(), p.patterns.len());
    for ch in &p.channels {
        let used_in: Vec<&str> = p
            .patterns
            .iter()
            .filter(|pat| pat.notes.iter().any(|n| n.channel == ch.iid))
            .map(|pat| pat.name.as_str())
            .collect();
        println!("  [{:2}] {:34} {:?}", ch.iid, ch.name, used_in);
    }
    Ok(())
}
