//! Structural changes sent to the running engine (adding channels and effects).
//!
//! Heavy objects (plugins, samples) are built on the UI side and moved in; whatever
//! they replace is sent back so it's dropped off the audio thread.

use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};

use std::sync::Arc;

use crate::sample::Sample;
use crate::{Channel, Effect, Source};

pub enum Command {
    /// Appends a channel (its [`crate::control::Strip`] should be set up first).
    AddChannel(Box<Channel>),
    /// Swaps a channel's sound source (e.g. loading a plugin into it, or clearing it).
    ReplaceSource { channel: usize, source: Box<Source> },
    /// Changes a channel's arpeggiator.
    SetArp { channel: usize, settings: crate::arp::ArpSettings },
    /// Starts (Some) or stops (None) input monitoring.
    /// Monitoring goes into mixer insert `insert` (0 = straight to the output).
    SetMonitor(Option<std::sync::Arc<crate::monitor::MonitorRing>>, usize),
    /// Changes an insert's built-in EQ/compressor.
    SetStrip { insert: usize, settings: Box<crate::channel_strip::StripSettings> },
    /// Inserts an effect into an insert's chain at `index` (clamped to the end).
    InsertEffect { insert: usize, index: usize, effect: Box<Effect> },
    RemoveEffect { insert: usize, index: usize },
    SetEffectMix { insert: usize, index: usize, mix: f32 },
    /// Plays (velocity > 0) or releases a note on a channel right now, for auditioning.
    Preview { channel: usize, key: u8, velocity: u8 },
    SetEffectBypass { insert: usize, index: usize, bypass: bool },
    /// Sets an automatable control (0..=1), e.g. from a MIDI controller.
    SetControl { target: crate::view::AutomationTarget, value: f32 },
    /// Changes a built-in effect's settings.
    SetBuiltinEffect { insert: usize, index: usize, params: Box<crate::effects::BuiltinEffect> },
    /// Replaces where an insert sends its output (inserts by index; 0 = master).
    /// Sets an insert's destinations and the send level (linear) to each.
    SetRoutes { insert: usize, routes: Vec<usize>, levels: Vec<f32> },
    /// Sets which inserts' compressors this insert keys (sidechain sends).
    SetSidechains { insert: usize, targets: Vec<usize> },
    /// Changes a channel's sampler settings (pitch, envelope, cut itself...).
    SetSampler { channel: usize, settings: crate::instrument::SamplerSettings },
    /// Changes a built-in synth's settings.
    SetSynth { channel: usize, params: Box<crate::instrument::SynthParams> },
    /// Plays a sample straight to the master (browser preview); `None` stops it.
    PreviewSample(Option<Arc<Sample>>),
}

/// Objects the engine no longer needs, to be dropped on the UI thread.
pub enum Garbage {
    Channel(Box<Channel>),
    Source(Box<Source>),
    Effect(Box<Effect>),
    Sample(Arc<Sample>),
    Monitor(Arc<crate::monitor::MonitorRing>),
}

pub struct CommandSender {
    tx: SyncSender<Command>,
    garbage: Receiver<Garbage>,
}

/// A cloneable sender for live note input from other threads (MIDI callbacks).
#[derive(Clone)]
pub struct NoteSender(SyncSender<Command>);

impl NoteSender {
    /// Plays (velocity > 0) or releases a note on `channel` immediately.
    pub fn note(&self, channel: usize, key: u8, velocity: u8) {
        let _ = self.0.try_send(Command::Preview { channel, key, velocity });
    }
}

impl CommandSender {
    /// Queues a change; it takes effect at the engine's next block.
    pub fn send(&self, command: Command) -> Result<(), String> {
        self.collect_garbage();
        match self.tx.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("the audio engine is busy; try again".into()),
            Err(TrySendError::Disconnected(_)) => Err("the audio engine has stopped".into()),
        }
    }

    pub fn note_sender(&self) -> NoteSender {
        NoteSender(self.tx.clone())
    }

    /// Drops whatever the engine has handed back (plugins shut their helpers down here).
    pub fn collect_garbage(&self) {
        while self.garbage.try_recv().is_ok() {}
    }
}

pub(crate) struct CommandReceiver {
    pub rx: Receiver<Command>,
    pub garbage: Sender<Garbage>,
}

pub(crate) fn link() -> (CommandSender, CommandReceiver) {
    let (tx, rx) = mpsc::sync_channel(64);
    let (gtx, grx) = mpsc::channel();
    (CommandSender { tx, garbage: grx }, CommandReceiver { rx, garbage: gtx })
}
