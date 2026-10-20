#include "plughost.h"

#include <juce_audio_processors/juce_audio_processors.h>
#include <juce_gui_basics/juce_gui_basics.h>

#import <AppKit/AppKit.h>

#include <algorithm>
#include <cstring>
#include <memory>

namespace
{
juce::AudioPluginFormatManager& formats()
{
    static auto manager = []
    {
        auto m = std::make_unique<juce::AudioPluginFormatManager>();
        juce::addDefaultFormatsToManager (*m);
        return m;
    }();
    return *manager;
}

void writeString (const juce::String& s, char* out, size_t capacity)
{
    if (out == nullptr || capacity == 0)
        return;
    auto utf8 = s.toRawUTF8();
    auto n = std::min (std::strlen (utf8), capacity - 1);
    std::memcpy (out, utf8, n);
    out[n] = '\0';
}

class EditorWindow final : public juce::DocumentWindow
{
public:
    EditorWindow (juce::AudioProcessorEditor* editor, const juce::String& title)
        : DocumentWindow (title, juce::Colours::black, DocumentWindow::closeButton)
    {
        setUsingNativeTitleBar (true);
        setContentOwned (editor, true);
        setResizable (editor->isResizable(), false);
        if (getWidth() <= 50 || getHeight() <= 50)
            setSize (600, 400);
        centreWithSize (getWidth(), getHeight());
        setVisible (true);
    }

    void closeButtonPressed() override { setVisible (false); }
};
} // namespace

// Set while KVNG itself changes a parameter (automation playback), so those changes
// aren't reported back as if the user had moved the control.
static thread_local bool hostIsSetting = false;

/// Collects parameter changes the user makes in the plugin's own window (latest value per
/// parameter), for KVNG's automation recording.
struct ParamWatcher : juce::AudioProcessorParameter::Listener
{
    juce::SpinLock lock;
    std::vector<std::pair<int, float>> changes;

    void parameterValueChanged (int index, float value) override
    {
        if (hostIsSetting)
            return;
        const juce::SpinLock::ScopedLockType guard (lock);
        for (auto& c : changes)
            if (c.first == index)
            {
                c.second = value;
                return;
            }
        if (changes.size() < 4096)
            changes.emplace_back (index, value);
    }

    void parameterGestureChanged (int, bool) override {}
};

struct KvngPlugin
{
    std::unique_ptr<juce::AudioPluginInstance> instance;
    std::unique_ptr<EditorWindow> editor;
    juce::AudioBuffer<float> scratch;
    juce::MidiBuffer midi;
    int maxBlock = 0;
    ParamWatcher watcher;

    ~KvngPlugin()
    {
        editor.reset();
        if (instance != nullptr)
        {
            for (auto* p : instance->getParameters())
                p->removeListener (&watcher);
            instance->releaseResources();
        }
    }
};

static KvngPlugin* finishLoad (const juce::PluginDescription& desc, double sampleRate, int maxBlock,
                               char* error, size_t errorLen)
{
    juce::String message;
    auto instance = formats().createPluginInstance (desc, sampleRate, maxBlock, message);
    if (instance == nullptr)
    {
        writeString (message.isEmpty() ? "plugin failed to load" : message, error, errorLen);
        return nullptr;
    }

    instance->enableAllBuses();
    instance->setRateAndBufferSizeDetails (sampleRate, maxBlock);
    instance->prepareToPlay (sampleRate, maxBlock);

    auto* plugin = new KvngPlugin();
    plugin->maxBlock = maxBlock;
    const auto channels = std::max (instance->getTotalNumInputChannels(), instance->getTotalNumOutputChannels());
    plugin->scratch.setSize (std::max (channels, 1), maxBlock);
    plugin->midi.ensureSize (1024);
    plugin->instance = std::move (instance);
    plugin->watcher.changes.reserve (64);
    for (auto* p : plugin->instance->getParameters())
        p->addListener (&plugin->watcher);
    return plugin;
}

static juce::AudioPluginFormat* findFormat (const juce::String& name)
{
    for (auto* f : formats().getFormats())
        if (f->getName() == name)
            return f;
    return nullptr;
}

extern "C" {

void kvng_host_init (void)
{
    [NSApp setActivationPolicy: NSApplicationActivationPolicyAccessory];
    juce::initialiseJuce_GUI();
    juce::MessageManager::getInstance()->setCurrentThreadAsMessageThread();
    formats();
}

void kvng_host_run_messages (int32_t ms)
{
    juce::MessageManager::getInstance()->runDispatchLoopUntil (ms);
}

void kvng_host_shutdown (void)
{
    juce::shutdownJuce_GUI();
}

KvngPlugin* kvng_plugin_load_vst3 (const char* bundlePath, const char* name, double sampleRate,
                                   int32_t maxBlock, char* error, size_t errorLen)
{
    auto* format = findFormat ("VST3");
    if (format == nullptr)
    {
        writeString ("VST3 hosting is not available", error, errorLen);
        return nullptr;
    }

    juce::OwnedArray<juce::PluginDescription> found;
    format->findAllTypesForFile (found, juce::String::fromUTF8 (bundlePath));
    if (found.isEmpty())
    {
        writeString (juce::String ("no VST3 plugin found in ") + bundlePath, error, errorLen);
        return nullptr;
    }

    const juce::PluginDescription* chosen = found.getFirst();
    if (name != nullptr)
        for (auto* d : found)
            if (d->name == juce::String::fromUTF8 (name))
                chosen = d;

    return finishLoad (*chosen, sampleRate, maxBlock, error, errorLen);
}

KvngPlugin* kvng_plugin_load_au (const char* type, const char* subtype, const char* manufacturer,
                                 double sampleRate, int32_t maxBlock, char* error, size_t errorLen)
{
    auto* format = findFormat ("AudioUnit");
    if (format == nullptr)
    {
        writeString ("Audio Unit hosting is not available", error, errorLen);
        return nullptr;
    }

    // JUCE identifies Audio Units as "AudioUnit:<category>/<type>,<subtype>,<manufacturer>".
    const auto identifier = juce::String ("AudioUnit:Unknown/") + type + "," + subtype + "," + manufacturer;
    juce::OwnedArray<juce::PluginDescription> found;
    format->findAllTypesForFile (found, identifier);
    if (found.isEmpty())
    {
        writeString ("Audio Unit " + identifier.fromFirstOccurrenceOf ("/", false, false) + " is not installed",
                     error, errorLen);
        return nullptr;
    }

    return finishLoad (*found.getFirst(), sampleRate, maxBlock, error, errorLen);
}

void kvng_plugin_free (KvngPlugin* plugin)
{
    delete plugin;
}

bool kvng_plugin_restore_vst3 (KvngPlugin* plugin, const uint8_t* component, size_t componentLen,
                               const uint8_t* controller, size_t controllerLen)
{
    // Repackage into the XML container JUCE's VST3 host reads.
    juce::XmlElement state ("VST3PluginState");
    state.createNewChildElement ("IComponent")
        ->addTextElement (juce::MemoryBlock (component, componentLen).toBase64Encoding());
    if (controller != nullptr && controllerLen > 0)
        state.createNewChildElement ("IEditController")
            ->addTextElement (juce::MemoryBlock (controller, controllerLen).toBase64Encoding());

    juce::MemoryBlock binary;
    juce::AudioProcessor::copyXmlToBinary (state, binary);
    plugin->instance->setStateInformation (binary.getData(), (int) binary.getSize());
    return true;
}

bool kvng_plugin_restore_au (KvngPlugin* plugin, const uint8_t* classInfo, size_t len)
{
    // JUCE's AU host accepts a ClassInfo property list directly.
    plugin->instance->setStateInformation (classInfo, (int) len);
    return true;
}

size_t kvng_plugin_save_state (KvngPlugin* plugin, uint8_t* out, size_t capacity)
{
    juce::MemoryBlock block;
    plugin->instance->getStateInformation (block);
    if (out != nullptr)
        std::memcpy (out, block.getData(), std::min (capacity, block.getSize()));
    return block.getSize();
}

bool kvng_plugin_load_state (KvngPlugin* plugin, const uint8_t* data, size_t len)
{
    plugin->instance->setStateInformation (data, (int) len);
    return true;
}

size_t kvng_plugin_name (KvngPlugin* plugin, char* out, size_t capacity)
{
    const auto name = plugin->instance->getName();
    writeString (name, out, capacity);
    return std::strlen (name.toRawUTF8());
}

int32_t kvng_plugin_num_inputs (KvngPlugin* plugin) { return plugin->instance->getTotalNumInputChannels(); }
int32_t kvng_plugin_num_outputs (KvngPlugin* plugin) { return plugin->instance->getTotalNumOutputChannels(); }
int32_t kvng_plugin_latency (KvngPlugin* plugin) { return plugin->instance->getLatencySamples(); }
bool kvng_plugin_accepts_midi (KvngPlugin* plugin) { return plugin->instance->acceptsMidi(); }

void kvng_plugin_process (KvngPlugin* plugin, float* const* channels, int32_t numChannels,
                          int32_t numSamples, const KvngMidiEvent* midi, int32_t numMidi)
{
    auto& buffer = plugin->scratch;
    numSamples = std::min (numSamples, plugin->maxBlock);
    buffer.setSize (buffer.getNumChannels(), numSamples, false, false, true);

    const auto ins = plugin->instance->getTotalNumInputChannels();
    for (int ch = 0; ch < buffer.getNumChannels(); ++ch)
    {
        if (ch < ins && ch < numChannels)
            buffer.copyFrom (ch, 0, channels[ch], numSamples);
        else
            buffer.clear (ch, 0, numSamples);
    }

    plugin->midi.clear();
    for (int i = 0; i < numMidi; ++i)
        plugin->midi.addEvent (midi[i].bytes, midi[i].len, midi[i].sample_offset);

    plugin->instance->processBlock (buffer, plugin->midi);

    const auto outs = plugin->instance->getTotalNumOutputChannels();
    for (int ch = 0; ch < numChannels; ++ch)
    {
        if (ch < outs)
            std::memcpy (channels[ch], buffer.getReadPointer (ch), sizeof (float) * (size_t) numSamples);
        else
            std::memset (channels[ch], 0, sizeof (float) * (size_t) numSamples);
    }
}

int32_t kvng_plugin_param_count (KvngPlugin* plugin)
{
    return plugin->instance->getParameters().size();
}

size_t kvng_plugin_param_name (KvngPlugin* plugin, int32_t index, char* out, size_t capacity)
{
    auto& params = plugin->instance->getParameters();
    if (index < 0 || index >= params.size())
        return 0;
    const auto name = params[index]->getName (64);
    writeString (name, out, capacity);
    return std::strlen (name.toRawUTF8());
}

float kvng_plugin_param_get (KvngPlugin* plugin, int32_t index)
{
    auto& params = plugin->instance->getParameters();
    return (index >= 0 && index < params.size()) ? params[index]->getValue() : 0.0f;
}

void kvng_plugin_param_set (KvngPlugin* plugin, int32_t index, float value)
{
    auto& params = plugin->instance->getParameters();
    if (index >= 0 && index < params.size())
    {
        hostIsSetting = true;
        params[index]->setValue (juce::jlimit (0.0f, 1.0f, value));
        hostIsSetting = false;
    }
}

size_t kvng_plugin_take_param_changes (KvngPlugin* plugin, int32_t* indices, float* values, size_t capacity)
{
    const juce::SpinLock::ScopedLockType guard (plugin->watcher.lock);
    auto& changes = plugin->watcher.changes;
    const auto n = std::min (capacity, changes.size());
    for (size_t i = 0; i < n; ++i)
    {
        indices[i] = (int32_t) changes[i].first;
        values[i] = changes[i].second;
    }
    changes.erase (changes.begin(), changes.begin() + (std::ptrdiff_t) n);
    return n;
}

static void bringWindowToFront (juce::Component* comp)
{
    if (comp == nullptr)
        return;
    comp->setVisible (true);
    comp->toFront (true);
    if (auto* peer = comp->getPeer())
    {
        if (auto* view = (NSView*) peer->getNativeHandle())
        {
            NSWindow* win = [view window];
            if (win != nil)
            {
                [win setAlphaValue: 1.0];
                [win makeKeyAndOrderFront: nil];
                [win orderFrontRegardless];
            }
        }
    }
    [NSApp activateIgnoringOtherApps: YES];
}

bool kvng_plugin_open_editor (KvngPlugin* plugin)
{
    [NSApp setActivationPolicy: NSApplicationActivationPolicyAccessory];

    if (plugin->editor != nullptr)
    {
        bringWindowToFront (plugin->editor.get());
        return true;
    }

    auto* editor = plugin->instance->createEditorIfNeeded();
    if (editor == nullptr)
        return false;

    plugin->editor = std::make_unique<EditorWindow> (editor, plugin->instance->getName());
    bringWindowToFront (plugin->editor.get());
    return true;
}

void kvng_plugin_close_editor (KvngPlugin* plugin)
{
    plugin->editor.reset();
}

bool kvng_plugin_snapshot (KvngPlugin* plugin, const char* pngPath, int32_t settleMs)
{
    if (! kvng_plugin_open_editor (plugin) || plugin->editor == nullptr)
        return false;

    auto* peer = plugin->editor->getPeer();
    if (peer == nullptr)
        return false;

    auto* view = (NSView*) peer->getNativeHandle();
    // Invisible to the user, but still laid out and drawn.
    [[view window] setAlphaValue: 0.0];
    juce::MessageManager::getInstance()->runDispatchLoopUntil (settleMs);

    bool ok = false;
    @autoreleasepool
    {
        const NSRect bounds = [view bounds];
        if (NSBitmapImageRep* rep = [view bitmapImageRepForCachingDisplayInRect: bounds])
        {
            [view cacheDisplayInRect: bounds toBitmapImageRep: rep];
            NSData* png = [rep representationUsingType: NSBitmapImageFileTypePNG properties: @{}];
            ok = [png writeToFile: [NSString stringWithUTF8String: pngPath] atomically: YES];
        }
    }
    kvng_plugin_close_editor (plugin);
    return ok;
}

static juce::String describeJson (const juce::PluginDescription& d)
{
    auto* o = new juce::DynamicObject();
    o->setProperty ("name", d.name);
    o->setProperty ("vendor", d.manufacturerName);
    o->setProperty ("category", d.category);
    o->setProperty ("is_instrument", d.isInstrument);
    o->setProperty ("format", d.pluginFormatName);
    o->setProperty ("id", d.fileOrIdentifier);
    o->setProperty ("version", d.version);
    o->setProperty ("inputs", d.numInputChannels);
    o->setProperty ("outputs", d.numOutputChannels);
    return juce::JSON::toString (juce::var (o), true);
}

void kvng_scan_list_audio_units (KvngScanCallback cb, void* user)
{
    if (auto* format = findFormat ("AudioUnit"))
    {
        for (auto& id : format->searchPathsForPlugins ({}, true, false))
        {
            auto* o = new juce::DynamicObject();
            o->setProperty ("id", id);
            cb (user, juce::JSON::toString (juce::var (o), true).toRawUTF8());
        }
    }
}

bool kvng_scan_describe (const char* formatName, const char* fileOrIdentifier,
                         KvngScanCallback cb, void* user, char* error, size_t errorLen)
{
    auto* format = findFormat (juce::String::fromUTF8 (formatName));
    if (format == nullptr)
    {
        writeString ("unknown plugin format", error, errorLen);
        return false;
    }
    juce::OwnedArray<juce::PluginDescription> found;
    format->findAllTypesForFile (found, juce::String::fromUTF8 (fileOrIdentifier));
    if (found.isEmpty())
    {
        writeString ("no plugins found (wrong architecture, or the plugin failed to load)", error, errorLen);
        return false;
    }
    for (auto* d : found)
        cb (user, describeJson (*d).toRawUTF8());
    return true;
}

} // extern "C"
