//! Live note input: MIDI keyboards and the computer keyboard.
//!
//! MIDI notes go from midir's thread straight into the engine's command queue (no UI
//! frame in between), aimed at the selected channel. A copy of each note is kept for
//! recording.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use engine::command::NoteSender;
use midir::{MidiInput, MidiInputConnection};

/// A note event seen by live input: key and velocity (0 = released).
pub type LiveNote = (u8, u8);

/// A controller (CC) move: MIDI channel, controller number, value 0..=127.
pub type LiveCc = (u8, u8, u8);

pub struct LiveInput {
    _connections: Vec<MidiInputConnection<()>>,
    pub port_names: Vec<String>,
    target: Arc<AtomicUsize>,
    sender: NoteSender,
    tx: Sender<LiveNote>,
    rx: Receiver<LiveNote>,
    cc_rx: Receiver<LiveCc>,
}

impl LiveInput {
    /// Connects to every MIDI input port present now.
    pub fn connect(sender: NoteSender, disabled: &[String]) -> LiveInput {
        let target = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let (cc_tx, cc_rx) = mpsc::channel();
        let mut connections = Vec::new();
        let mut port_names = Vec::new();
        if let Ok(probe) = MidiInput::new("KVNG Studio") {
            for port in probe.ports() {
                let name = probe.port_name(&port).unwrap_or_else(|_| "MIDI input".into());
                if disabled.contains(&name) {
                    continue;
                }
                let Ok(input) = MidiInput::new("KVNG Studio") else { continue };
                let (sender, target, tx, cc_tx) = (sender.clone(), target.clone(), tx.clone(), cc_tx.clone());
                let conn = input.connect(
                    &port,
                    "kvng-in",
                    move |_, msg, _| {
                        if let [s, cc, value, ..] = msg
                            && s & 0xF0 == 0xB0
                        {
                            let _ = cc_tx.send((s & 0x0F, cc & 0x7F, value & 0x7F));
                        }
                        if let Some((key, velocity)) = parse(msg) {
                            sender.note(target.load(Ordering::Relaxed), key, velocity);
                            let _ = tx.send((key, velocity));
                        }
                    },
                    (),
                );
                if let Ok(c) = conn {
                    connections.push(c);
                    port_names.push(name);
                }
            }
        }
        LiveInput { _connections: connections, port_names, target, sender, tx, rx, cc_rx }
    }

    /// Points live input at `channel`.
    pub fn set_target(&self, channel: usize) {
        self.target.store(channel, Ordering::Relaxed);
    }

    /// Plays a note from the computer keyboard.
    pub fn note(&self, key: u8, velocity: u8) {
        self.sender.note(self.target.load(Ordering::Relaxed), key, velocity);
        let _ = self.tx.send((key, velocity));
    }

    /// Notes played since the last call.
    pub fn drain(&self) -> Vec<LiveNote> {
        self.rx.try_iter().collect()
    }

    /// Controller moves since the last call.
    pub fn drain_cc(&self) -> Vec<LiveCc> {
        self.cc_rx.try_iter().collect()
    }
}

/// Note-on/off from a raw MIDI message (note-on with velocity 0 is a release).
fn parse(msg: &[u8]) -> Option<LiveNote> {
    match msg {
        [s, key, vel, ..] if s & 0xF0 == 0x90 => Some((*key & 0x7F, *vel & 0x7F)),
        [s, key, ..] if s & 0xF0 == 0x80 => Some((*key & 0x7F, 0)),
        _ => None,
    }
}

/// FL Studio's typing-keyboard layout: the bottom letter row plays C5–B5 (white keys
/// Z X C V B N M, black keys S D G H J); the top row plays C6–B6 (Q W E R T Y U, with
/// 2 3 5 6 7). Returns the MIDI key.
pub fn typing_key(key: eframe::egui::Key) -> Option<u8> {
    use eframe::egui::Key::*;
    let offset = match key {
        Z => 0,
        S => 1,
        X => 2,
        D => 3,
        C => 4,
        V => 5,
        G => 6,
        B => 7,
        H => 8,
        N => 9,
        J => 10,
        M => 11,
        Q => 12,
        Num2 => 13,
        W => 14,
        Num3 => 15,
        E => 16,
        R => 17,
        Num5 => 18,
        T => 19,
        Num6 => 20,
        Y => 21,
        Num7 => 22,
        U => 23,
        _ => return None,
    };
    Some(60 + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_note_messages() {
        assert_eq!(parse(&[0x90, 60, 100]), Some((60, 100)));
        assert_eq!(parse(&[0x93, 61, 0]), Some((61, 0)));
        assert_eq!(parse(&[0x80, 62, 40]), Some((62, 0)));
        assert_eq!(parse(&[0xB0, 64, 127]), None);
    }
}
