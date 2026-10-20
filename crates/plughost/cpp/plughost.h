// C interface between KVNG Studio (Rust) and the JUCE-based plugin host.
// Everything JUCE-related stays behind this header.
#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct KvngPlugin KvngPlugin;

typedef struct {
    int32_t sample_offset; // position within the current block
    uint8_t bytes[3];      // raw MIDI message
    uint8_t len;           // 1..=3
} KvngMidiEvent;

// Must be called once, on the main thread, before any other call.
void kvng_host_init(void);
// Runs JUCE's message loop for `ms` milliseconds. Only needed when nothing
// else (e.g. the UI) is running the macOS main run loop.
void kvng_host_run_messages(int32_t ms);
void kvng_host_shutdown(void);

// Loading and state calls must be made on the main thread.
// `name` picks a plugin when a bundle contains several; may be NULL.
KvngPlugin* kvng_plugin_load_vst3(const char* bundle_path, const char* name,
                                  double sample_rate, int32_t max_block,
                                  char* error, size_t error_len);
// Four-character codes, e.g. "aumu", "k105", "Aufb".
KvngPlugin* kvng_plugin_load_au(const char* type, const char* subtype, const char* manufacturer,
                                double sample_rate, int32_t max_block,
                                char* error, size_t error_len);
void kvng_plugin_free(KvngPlugin* plugin);

bool kvng_plugin_restore_vst3(KvngPlugin* plugin,
                              const uint8_t* component, size_t component_len,
                              const uint8_t* controller, size_t controller_len);
bool kvng_plugin_restore_au(KvngPlugin* plugin, const uint8_t* class_info, size_t len);

// Writes the plugin's current state in JUCE's format. Returns the full size;
// call with out == NULL to query it.
size_t kvng_plugin_save_state(KvngPlugin* plugin, uint8_t* out, size_t capacity);
bool kvng_plugin_load_state(KvngPlugin* plugin, const uint8_t* data, size_t len);

size_t kvng_plugin_name(KvngPlugin* plugin, char* out, size_t capacity);
int32_t kvng_plugin_num_inputs(KvngPlugin* plugin);
int32_t kvng_plugin_num_outputs(KvngPlugin* plugin);
int32_t kvng_plugin_latency(KvngPlugin* plugin);
bool kvng_plugin_accepts_midi(KvngPlugin* plugin);

// Real-time safe once loaded. `channels` holds `num_channels` buffers of
// `num_samples` floats: inputs on entry, outputs on return.
void kvng_plugin_process(KvngPlugin* plugin, float* const* channels, int32_t num_channels,
                         int32_t num_samples, const KvngMidiEvent* midi, int32_t num_midi);

// Parameters (normalised 0..1). Setting is allowed from the audio thread just before
// processing, which is how automation is applied.
int32_t kvng_plugin_param_count(KvngPlugin* plugin);
size_t kvng_plugin_param_name(KvngPlugin* plugin, int32_t index, char* out, size_t capacity);
float kvng_plugin_param_get(KvngPlugin* plugin, int32_t index);
void kvng_plugin_param_set(KvngPlugin* plugin, int32_t index, float value);
/* Parameter changes the user made in the plugin's window since the last call (latest value
   per parameter). Fills up to `capacity` entries; returns how many. */
size_t kvng_plugin_take_param_changes(KvngPlugin* plugin, int32_t* indices, float* values, size_t capacity);

bool kvng_plugin_open_editor(KvngPlugin* plugin);
void kvng_plugin_close_editor(KvngPlugin* plugin);

// Renders the plugin's editor off-screen for `settle_ms` and writes it as PNG.
// GPU-drawn editors may come out blank; callers should check the image.
bool kvng_plugin_snapshot(KvngPlugin* plugin, const char* png_path, int32_t settle_ms);

// ---- Scanning --------------------------------------------------------------
// Results are delivered as one JSON object per call of `cb`.
typedef void (*KvngScanCallback)(void* user, const char* json);

// Lists installed Audio Units as JSON {"id": "..."} without loading them.
void kvng_scan_list_audio_units(KvngScanCallback cb, void* user);
// Describes each plugin in a VST3 bundle or an Audio Unit identifier (loads it).
// `format` is "VST3" or "AudioUnit".
bool kvng_scan_describe(const char* format, const char* file_or_identifier,
                        KvngScanCallback cb, void* user, char* error, size_t error_len);

#ifdef __cplusplus
}
#endif
