# KVNG Studio vs FL Studio — feature parity (2026-10-11)

Compared area by area against FL Studio's public manual. **Done** = built and tested in
KVNG; **Partial** = works but narrower than FL; **Missing** = not built. "Can't" = needs
something unavailable to this project (FL's own code or content, hardware, accounts).

Scope note: FL Studio ships ~80 Image-Line instruments/effects. KVNG can't include those
(they're Image-Line's), so it plays third-party VST3/AU plugins and has its own built-ins.

## Project & files
| FL feature | KVNG | Notes |
|---|---|---|
| Open .flp projects | Done | Channels, patterns, playlist, mixer, plugin states (VST3/AU), FL effects via adapters where possible |
| Save / Save as / New / templates | Done | `.kvng` JSON; FL-style default template |
| Autosave, backups, crash recovery | Done | |
| Export WAV (16/24/32), stems | Done | Range (song/loop), tail, normalize, dither, clip report |
| Export FLAC | Done | 16/24-bit |
| Export MP3 / OGG | Missing | Needs an encoder dependency (LAME/Vorbis) |
| Import / export MIDI files | Done | Per-track channels on import; pattern export |
| Collect samples / relink missing | Done | |
| Video export, ZIP project export | Missing | |

## Browser
| Search, favourites, preview with waveform, drag to rack/playlist | Done |
|---|---|
| Plugin database with pictures, favourites, rescan | Done |
| Tags / BPM-key detection of samples | Missing |

## Channel rack & step sequencer
| FL feature | KVNG | Notes |
|---|---|---|
| Step sequencer, pattern length, swing | Done | |
| Graph editor (velocity/pan/pitch/mod per step) | Done | FL also has release/fine pitch/shift lanes — Missing |
| Channel volume/pan/mute/solo, mixer track number | Done | |
| Channel filter groups | Done | |
| Replace/drag samples & plugins onto channels | Done | |
| Channel settings: sampler (envelope, pitch, reverse, cut, filter) | Partial | Filter added (LP/HP/BP); no LFOs or time-stretch modes |
| Channel arpeggiator | Done | FL also has chord/arp presets — Partial |
| Layer / Audio clip / Automation channel types | Partial | Audio clips & automation yes; Layer channels Missing |
| Render channel to audio (bounce) | Done | |

## Piano roll
| FL feature | KVNG | Notes |
|---|---|---|
| Draw/paint/select/slice/delete tools, snap, ghost notes | Done | |
| Velocity, pan, Mod X/Y lanes | Done | Note release / fine pitch lanes Missing |
| Quantize (strength), humanize, strum (live preview), arpeggiate, chop, glue | Done | |
| Scale highlight + snap, chord stamps | Done | |
| Piano feel (velocity contour) | Done | |
| Slide notes | Done | KVNG Synth, sampler, and plugins (pitch bend; range set per channel) |
| Note colours = MIDI channels | Missing | |
| Flip (time/pitch), randomizer (scale-aware), LFO tool | Done | |
| Riff machine, limiter | Missing | |

## Playlist
| FL feature | KVNG | Notes |
|---|---|---|
| Pattern / audio / automation clips, multi-select, copy, slice | Done | |
| Clip gain, fades (equal-power), reverse, trim, slip | Done | |
| Markers, song loop, tempo changes, time-signature changes | Done | |
| Insert/delete/duplicate bars | Done | |
| Track folders, colours, rename, mute | Done | |
| Make unique, clip colours | Done | |
| Audio clip stretch to tempo | Done | Clip menu → Stretch (to project tempo or N bars), keeps pitch; offline (not automatic on tempo change) |
| Performance mode, playlist arrangements (multiple) | Missing | |

## Mixer
| FL feature | KVNG | Notes |
|---|---|---|
| 125 inserts + master, routing, send levels | Done | |
| Sidechain to compressors | Done | Built-in channel compressor only; plugins' sidechain inputs Missing |
| 10 effect slots, bypass, mix, effect chains | Done | |
| Built-in EQ + compressor per strip, GR meter | Done | KVNG addition (from your roadmap) |
| Stereo separation, swap, polarity | Done | |
| Insert latency compensation (PDC) | Done | Automatic |
| Record into a mixer track (arm), monitor through its effects | Done | |
| Multi-output plugins | Missing | |

## Automation, recording, MIDI
| FL feature | KVNG | Notes |
|---|---|---|
| Automation clips with curves (tension) | Done | |
| Create automation from any control | Done | |
| Record knob moves into automation | Done | KVNG controls tested; plugin-window knobs implemented (no-echo tested with Kia), not tested with a real mouse |
| MIDI learn / controller links | Done | |
| Note recording (MIDI keyboard, typing keyboard), count-in, metronome | Done | |
| Audio recording, input monitoring, latency compensation | Done (untested live) | Needs a microphone to verify |
| MIDI out to hardware | Done (untested) | Needs a MIDI device |

## Built-in sounds & effects
| FL | KVNG |
|---|---|
| FL instruments (Sytrus, FLEX, 3xOsc, FPC, Slicex…) | Can't (Image-Line's). KVNG Synth + presets, drums, slicer, sampler instead |
| FL effects (Parametric EQ 2, Fruity Limiter, Reverb 2…) | Partial via adapters for a few; KVNG EQ/reverb/delay/compressor/chorus/flanger/phaser/distortion/filter/gate |
| Edison (audio editor) | Done (KVNG audio editor: edit, stretch, pitch, slice) |
| Patcher, Fruity Wrapper settings | Missing |

## Platform & distribution
| Item | Status |
|---|---|
| macOS universal app (Apple Silicon + Intel), Intel-only plugins via Rosetta | Done |
| Notarization | Can't here (Apple Developer account) |
| Windows build | Can't here (needs Windows PC/CI; plugin hosting is POSIX-specific) |
| JUCE licence for distribution | AGPLv3 source release selected; see LICENSE and docs/RELEASE.md |

## Biggest remaining gaps (buildable)
1. MP3/OGG export (needs an encoder dependency).
2. Multi-output plugins; plugins' own sidechain inputs.
3. Layer channels; sampler LFOs.
4. Note release / fine-pitch lanes.
5. Riff machine; playlist performance mode; multiple arrangements.
