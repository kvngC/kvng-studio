cmake_minimum_required(VERSION 3.22)
project(kvng_plughost VERSION 0.1.0 LANGUAGES C CXX OBJC OBJCXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

if(NOT JUCE_DIR)
    message(FATAL_ERROR "Pass -DJUCE_DIR=<path to JUCE>")
endif()
add_subdirectory(${JUCE_DIR} juce EXCLUDE_FROM_ALL)

# JUCE modules are compiled straight into this static library, which is
# linked into the Rust binary.
add_library(kvng_plughost STATIC plughost.mm)

target_link_libraries(kvng_plughost
    PRIVATE
        juce::juce_audio_processors
        juce::juce_gui_basics
        juce::juce_recommended_config_flags
        juce::juce_recommended_warning_flags)

target_compile_definitions(kvng_plughost
    PRIVATE
        JUCE_GLOBAL_MODULE_SETTINGS_INCLUDED=1
        JUCE_STANDALONE_APPLICATION=0
        JUCE_PLUGINHOST_VST3=1
        JUCE_PLUGINHOST_AU=1
        JUCE_PLUGINHOST_LV2=0
        JUCE_PLUGINHOST_LADSPA=0
        JUCE_MODAL_LOOPS_PERMITTED=1
        JUCE_WEB_BROWSER=0
        JUCE_USE_CURL=0
        JUCE_DISPLAY_SPLASH_SCREEN=0)

install(TARGETS kvng_plughost ARCHIVE DESTINATION lib)
