cmake_minimum_required(VERSION 3.19)
project(ActiasLuau LANGUAGES CXX)

# Luau's own tree, fetched by build.sh; only the libraries build.
set(LUAU_BUILD_CLI OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_WEB OFF CACHE BOOL "" FORCE)
add_subdirectory(vendor/luau)

# One source, two consumers. Service.cpp holds the language service; each
# target below wraps it for the caller that loads it, and neither may
# answer a question differently from the other.
#
#   Actias.Luau      wasm, loaded by the workbench in actias-web
#   actias-luau      native command, run by actias-cli
#
# EMSCRIPTEN is set by emcmake, so a plain cmake run builds only the
# command and needs no emscripten toolchain installed at all.

if(NOT EMSCRIPTEN)
    add_executable(actias-luau Service.cpp Cli.cpp)
    target_compile_features(actias-luau PRIVATE cxx_std_17)
    target_link_libraries(actias-luau PRIVATE Luau.Analysis)
    # The analysis reports failure by throwing; emscripten is the target
    # that turns exceptions off by default, but say it in both places.
    target_compile_options(actias-luau PRIVATE -fexceptions)
endif()

if(EMSCRIPTEN)
    add_executable(Actias.Luau Service.cpp)
    target_compile_features(Actias.Luau PRIVATE cxx_std_17)
    target_link_libraries(Actias.Luau PRIVATE Luau.Analysis)
    target_compile_options(Actias.Luau PRIVATE -fexceptions)
    target_link_options(Actias.Luau PRIVATE
        -fexceptions
        # One .js with the wasm embedded, the shape the worker already loads.
        -sSINGLE_FILE=1
        -sALLOW_MEMORY_GROWTH=1
        "-sEXPORTED_FUNCTIONS=[\"_setFile\",\"_removeFile\",\"_checkScript\",\"_autocompleteScript\",\"_hoverScript\",\"_definitionScript\",\"_signatureScript\",\"_semanticScript\"]"
        "-sEXPORTED_RUNTIME_METHODS=[\"ccall\",\"cwrap\"]"
    )
endif()
