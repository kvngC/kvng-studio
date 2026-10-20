//! Prints a readable overview of an FLP project: `cargo run -p flp --example summary -- file.flp`

use flp::{ItemSource, PluginFormat, PluginState, Project, WrappedPlugin};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: summary <file.flp>")?;
    let p: Project = flp::read(path)?;

    println!("FL {} | {} BPM | {} PPQ | {}/{}", p.version, p.tempo, p.ppq, p.time_signature.0, p.time_signature.1);

    println!("\nChannels ({}):", p.channels.len());
    for ch in &p.channels {
        let plugin = ch.plugin.as_ref().filter(|pl| !pl.internal_name.is_empty());
        let source = match (plugin, &ch.sample_path) {
            (Some(pl), _) if pl.is_third_party() => {
                let w = pl.wrapped.as_ref().unwrap();
                describe_wrapped(w)
            }
            (Some(pl), _) => format!("FL native {:?} ({}B data)", pl.internal_name, pl.data.len()),
            (None, Some(path)) => format!("sample {path}"),
            (None, None) => "empty".into(),
        };
        println!(
            "  [{:2}] {:?} {:32} -> insert {:?}  vol {:.2} pan {:+.2}  {}",
            ch.iid,
            ch.kind,
            format!("{:?}", ch.name),
            ch.insert.unwrap_or(0),
            ch.volume.unwrap_or(0.0),
            ch.pan.unwrap_or(0.0),
            source
        );
    }

    let notes: usize = p.patterns.iter().map(|pat| pat.notes.len()).sum();
    println!("\nPatterns ({}, {notes} notes):", p.patterns.len());
    for pat in &p.patterns {
        println!("  [{:2}] {:16} notes={}", pat.iid, format!("{:?}", pat.name), pat.notes.len());
    }

    for arr in &p.arrangements {
        let tracks: std::collections::BTreeSet<u16> = arr.items.iter().map(|it| it.track).collect();
        let patterns = arr.items.iter().filter(|it| matches!(it.source, ItemSource::Pattern(_))).count();
        let end = arr.items.iter().map(|it| it.position + it.length).max().unwrap_or(0);
        println!(
            "\nArrangement {:?}: {} items ({} pattern clips) on {} tracks, ends at bar {:.1}",
            arr.name,
            arr.items.len(),
            patterns,
            tracks.len(),
            end as f64 / (p.ppq as f64 * 4.0) + 1.0
        );
    }

    let used: Vec<_> = p.inserts.iter().filter(|i| !i.slots.is_empty() || i.name.is_some()).collect();
    println!("\nMixer: {} inserts", p.inserts.len());
    for ins in used {
        let label = if ins.index == 0 { "Master".to_string() } else { format!("Insert {}", ins.index) };
        let slots: Vec<String> = ins
            .slots
            .iter()
            .map(|s| {
                let detail = match &s.plugin.wrapped {
                    Some(w) => describe_wrapped(w),
                    None => format!("FL native {:?} ({}B data)", s.plugin.internal_name, s.plugin.data.len()),
                };
                format!("slot {}: {}", s.index + 1, detail)
            })
            .collect();
        println!("  {label} {:?}: {}", ins.name.as_deref().unwrap_or(""), slots.join(" | "));
    }
    Ok(())
}

fn describe_wrapped(w: &WrappedPlugin) -> String {
    let format = match w.format() {
        PluginFormat::Vst3 { .. } => format!("VST3 {}", w.path.as_deref().unwrap_or("?")),
        PluginFormat::AudioUnit { kind, subtype, manufacturer } => format!("AU {kind}/{subtype}/{manufacturer}"),
        PluginFormat::Other => "unknown format".into(),
    };
    let state = match w.decoded_state() {
        Some(PluginState::Vst3 { component, controller }) => {
            format!("component state {}B, controller {}B", component.len(), controller.map_or(0, |c| c.len()))
        }
        Some(PluginState::AudioUnit { class_info }) => format!("ClassInfo plist {}B", class_info.len()),
        Some(PluginState::Unknown(b)) => format!("undecoded state {}B", b.len()),
        None => "no state".into(),
    };
    format!("{:?} [{format}] {state}", w.name.as_deref().unwrap_or("?"))
}
