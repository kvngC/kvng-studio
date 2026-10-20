//! Sending notes to external MIDI devices (FL's "MIDI Out" channel).
//!
//! The audio thread only pushes messages into a bounded queue; a dedicated thread owns
//! the MIDI connection and sends them.

use std::sync::mpsc::{self, SyncSender, TrySendError};

use midir::MidiOutput;

/// Names of the MIDI output ports present now.
pub fn ports() -> Vec<String> {
    let Ok(out) = MidiOutput::new("KVNG Studio") else { return Vec::new() };
    out.ports().iter().filter_map(|p| out.port_name(p).ok()).collect()
}

/// A connection to one output port.
pub struct MidiOut {
    tx: SyncSender<[u8; 3]>,
    pub port: String,
}

impl MidiOut {
    /// Opens `port` (by name; the first port when empty).
    pub fn open(port: &str) -> Result<MidiOut, String> {
        let out = MidiOutput::new("KVNG Studio").map_err(|e| e.to_string())?;
        let ports = out.ports();
        let chosen = ports
            .iter()
            .find(|p| port.is_empty() || out.port_name(p).is_ok_and(|n| n == port))
            .ok_or_else(|| if port.is_empty() { "no MIDI output devices (connect one, or turn on the IAC Driver in Audio MIDI Setup)".to_string() } else { format!("MIDI output {port:?} not found") })?;
        let name = out.port_name(chosen).unwrap_or_default();
        let mut conn = out.connect(chosen, "kvng-out").map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::sync_channel::<[u8; 3]>(1024);
        std::thread::spawn(move || {
            while let Ok(msg) = rx.recv() {
                let _ = conn.send(&msg);
            }
            // Leave nothing hanging on the device when the channel goes away.
            for ch in 0..16u8 {
                let _ = conn.send(&[0xB0 | ch, 123, 0]);
            }
        });
        Ok(MidiOut { tx, port: name })
    }

    /// Queues a message (real-time safe: drops it if the queue is full).
    pub fn send(&self, msg: [u8; 3]) {
        if let Err(TrySendError::Full(_)) = self.tx.try_send(msg) {
            // Dropping beats blocking the audio thread.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_a_port_or_explains_why_not() {
        match MidiOut::open("") {
            Ok(out) => {
                assert!(ports().contains(&out.port));
                out.send([0x90, 60, 0]);
            }
            Err(e) => assert!(!e.is_empty()),
        }
        assert!(MidiOut::open("no such device 12345").is_err());
    }
}
