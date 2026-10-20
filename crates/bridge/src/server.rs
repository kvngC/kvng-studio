//! The helper process: hosts exactly one plugin on behalf of KVNG Studio.
//!
//! Started as `kvng-plugin-host <socket> <shm> <request-sem> <done-sem>`.

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread::JoinHandle;

use plughost::{Host, MidiEvent, Plugin, Processor};

use crate::protocol::{self, Request, Response};
use crate::shm::{self, CHANNELS, Semaphore, SharedBlock};

struct Audio {
    block: Arc<SharedBlock>,
    request: Arc<Semaphore>,
    done: Arc<Semaphore>,
}

pub fn run(args: &[String]) -> Result<(), String> {
    let [socket, shm_name, req_name, done_name] = args else {
        return Err("usage: kvng-plugin-host <socket> <shm> <request-sem> <done-sem>".into());
    };

    // Open the shared resources before connecting: the client unlinks their
    // names as soon as we connect.
    let audio = Audio {
        block: Arc::new(SharedBlock::open(shm_name).map_err(|e| e.to_string())?),
        request: Arc::new(Semaphore::open(req_name).map_err(|e| e.to_string())?),
        done: Arc::new(Semaphore::open(done_name).map_err(|e| e.to_string())?),
    };
    if audio.block.get().layout.load(Ordering::SeqCst) != shm::layout_id() {
        eprintln!("kvng-plugin-host: built for a different version of KVNG Studio");
        std::process::exit(shm::EXIT_OUTDATED);
    }
    let mut control = UnixStream::connect(socket).map_err(|e| format!("connect {socket}: {e}"))?;

    let host = Host::init();

    // Requests arrive on a reader thread; JUCE calls must happen on this (main) thread.
    let (tx, rx) = mpsc::channel::<Option<Request>>();
    let mut reader = control.try_clone().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        loop {
            let msg = protocol::recv::<_, Request>(&mut reader).ok();
            let stop = msg.is_none() || msg == Some(Request::Shutdown);
            if tx.send(msg).is_err() || stop {
                break;
            }
        }
    });

    let mut plugin: Option<Plugin> = None;
    let mut audio_thread: Option<JoinHandle<()>> = None;

    'main: loop {
        host.run_messages(5);
        while let Ok(msg) = rx.try_recv() {
            let Some(req) = msg else { break 'main }; // client went away
            if req == Request::Shutdown {
                let _ = protocol::send(&mut control, &Response::Ok);
                break 'main;
            }
            let response = handle(&host, req, &mut plugin, &audio, &mut audio_thread);
            if protocol::send(&mut control, &response).is_err() {
                break 'main;
            }
        }
    }

    audio.block.get().quit.store(1, Ordering::SeqCst);
    audio.request.post();
    if let Some(t) = audio_thread {
        let _ = t.join();
    }
    drop(plugin);
    Ok(())
}

fn handle(host: &Host, req: Request, plugin: &mut Option<Plugin>, audio: &Audio, audio_thread: &mut Option<JoinHandle<()>>) -> Response {
    let result = match req {
        Request::LoadVst3 { path, name, sample_rate, max_block } => {
            host.load_vst3(&path, name.as_deref(), sample_rate, max_block as usize).map(|p| loaded(p, plugin, audio, audio_thread))
        }
        Request::LoadAu { kind, subtype, manufacturer, sample_rate, max_block } => host
            .load_au(&kind, &subtype, &manufacturer, sample_rate, max_block as usize)
            .map(|p| loaded(p, plugin, audio, audio_thread)),
        other => match plugin.as_mut() {
            None => Err("no plugin loaded".to_string()),
            Some(p) => Ok(match other {
                Request::RestoreVst3 { component, controller } => {
                    p.restore_vst3(&component, controller.as_deref());
                    after_state_change(p, audio)
                }
                Request::RestoreAu { class_info } => {
                    p.restore_au(&class_info);
                    after_state_change(p, audio)
                }
                Request::SaveState => Response::State(p.save_state()),
                Request::LoadState(state) => {
                    p.load_state(&state);
                    after_state_change(p, audio)
                }
                Request::OpenEditor => {
                    if p.open_editor() {
                        Response::Ok
                    } else {
                        Response::Error("plugin has no editor".into())
                    }
                }
                Request::CloseEditor => {
                    p.close_editor();
                    Response::Ok
                }
                Request::ListParams => Response::Params(p.parameters()),
                Request::TakeParamChanges => Response::ParamChanges(p.take_param_changes()),
                Request::LoadVst3 { .. } | Request::LoadAu { .. } | Request::Shutdown => unreachable!(),
            }),
        },
    };
    result.unwrap_or_else(Response::Error)
}

fn after_state_change(p: &Plugin, audio: &Audio) -> Response {
    shm::store_latency(audio.block.get(), p.latency_samples());
    Response::Ok
}

fn loaded(p: Plugin, slot: &mut Option<Plugin>, audio: &Audio, audio_thread: &mut Option<JoinHandle<()>>) -> Response {
    let response = Response::Loaded {
        name: p.name(),
        inputs: p.num_inputs() as u32,
        outputs: p.num_outputs() as u32,
        latency: p.latency_samples() as u32,
        accepts_midi: p.accepts_midi(),
    };
    shm::store_latency(audio.block.get(), p.latency_samples());
    let processor = p.processor();
    *slot = Some(p);
    let (block, request, done) = (audio.block.clone(), audio.request.clone(), audio.done.clone());
    *audio_thread = Some(std::thread::spawn(move || audio_loop(processor, &block, &request, &done)));
    response
}

/// Waits for a block from the client, processes it in place, and signals completion.
fn audio_loop(mut processor: Processor, block: &SharedBlock, request: &Semaphore, done: &Semaphore) {
    let mut midi: Vec<MidiEvent> = Vec::with_capacity(shm::MAX_MIDI);
    loop {
        request.wait();
        let b = block.get();
        if b.quit.load(Ordering::SeqCst) != 0 {
            break;
        }
        let n = (b.num_samples.load(Ordering::Acquire) as usize).min(shm::MAX_BLOCK);
        let m = (b.num_midi.load(Ordering::Acquire) as usize).min(shm::MAX_MIDI);
        midi.clear();
        midi.extend(b.midi[..m].iter().map(|e| MidiEvent { sample_offset: e.offset, bytes: e.bytes, len: e.len }));

        let np = (b.num_params.load(Ordering::Acquire) as usize).min(shm::MAX_PARAMS);
        for &(index, value) in &b.params[..np] {
            processor.set_param(index, value);
        }
        let [l, r] = &mut b.audio;
        let mut chans: [&mut [f32]; CHANNELS] = [&mut l[..n], &mut r[..n]];
        processor.process(&mut chans, &midi);
        shm::store_latency(b, processor.latency_samples());
        done.post();
    }
}
