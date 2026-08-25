cmake_minimum_required(VERSION 3.19)
project(ActiasLuauWeb LANGUAGES CXX)

# Luau's own tree, fetched by build.sh; only the libraries build.
set(LUAU_BUILD_CLI OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(LUAU_BUILD_WEB OFF CACHE BOOL "" FORCE)
add_subdirectory(vendor/luau)

add_executable(Actias.Luau Web.cpp)
target_compile_features(Actias.Luau PRIVATE cxx_std_17)
target_link_libraries(Actias.Luau PRIVATE Luau.Analysis)

# The analysis throws through C++ exceptions, which emscripten disables
# unless asked; upstream's web target sets the same flag.
target_compile_options(Actias.Luau PRIVATE -fexceptions)
target_link_options(Actias.Luau PRIVATE
    -fexceptions
    # One .js with the wasm embedded, the shape the worker already loads.
    -sSINGLE_FILE=1
    -sALLOW_MEMORY_GROWTH=1
    "-sEXPORTED_FUNCTIONS=[\"_setFile\",\"_removeFile\",\"_checkScript\",\"_autocompleteScript\",\"_hoverScript\",\"_definitionScript\",\"_signatureScript\",\"_semanticScript\"]"
    "-sEXPORTED_RUNTIME_METHODS=[\"ccall\",\"cwrap\"]"
)
