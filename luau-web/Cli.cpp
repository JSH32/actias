// The language service as a process, which is the only shape the cli can
// use it in.
//
// Linking Luau's Analysis into actias-cli is not possible: the cli
// already embeds a Luau through mlua for `actias test`, and mlua vendors
// its own version (luau709 against this tree's 0.735). Two Luau trees in
// one binary redefine every Luau::Ast symbol and the linker refuses. A
// separate process has its own address space, so the collision cannot
// arise and the cli needs no C++ toolchain to build.
//
// Two ways in, the same service behind both:
//
//   actias-luau <root> <module>...   check once and exit, for humans
//   actias-luau                      serve requests on stdin, for the cli
//
// The served protocol carries the same ops the workbench's worker sends
// the wasm build (see actias-web/public/luau/checker.js), so the editor,
// the language server and `actias check` ask the one implementation the
// same questions. Requests are framed by byte length rather than quoted,
// which keeps sources containing newlines or quotes from needing an
// escaping scheme, and keeps a JSON parser out of this file entirely:
//
//   set <pathLen> <textLen>\n<path><text>      no reply
//   remove <pathLen>\n<path>                   no reply
//   check <pathLen>\n<path>                    one json line
//   complete <pathLen> <line> <col>\n<path>    one json line
//   hover <pathLen> <line> <col>\n<path>       one json line
//   definition <pathLen> <line> <col>\n<path>  one json line
//   signature <pathLen> <line> <col>\n<path>   one json line
//   semantic <pathLen>\n<path>                 one json line
//   quit\n
//
// Positions are one-based, as everywhere else in the service. A reply of
// `null` means the service had nothing to say, not that it failed.

#include <cstdio>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

extern "C" void setFile(const char* name, const char* text);
extern "C" void removeFile(const char* name);
extern "C" const char* checkScript(const char* module);
extern "C" const char* autocompleteScript(const char* module, int line, int column);
extern "C" const char* hoverScript(const char* module, int line, int column);
extern "C" const char* definitionScript(const char* module, int line, int column);
extern "C" const char* signatureScript(const char* module, int line, int column);
extern "C" const char* semanticScript(const char* module);

static bool readFile(const std::string& path, std::string& out)
{
    std::ifstream file(path, std::ios::binary);
    if (!file)
        return false;

    std::ostringstream buffer;
    buffer << file.rdbuf();
    out = buffer.str();
    return true;
}

// Reads exactly `length` bytes, which is the point of the framing: the
// payload is never scanned for a terminator it might legitimately hold.
static bool readExactly(std::size_t length, std::string& out)
{
    out.resize(length);
    if (length == 0)
        return true;
    std::cin.read(&out[0], static_cast<std::streamsize>(length));
    return static_cast<std::size_t>(std::cin.gcount()) == length;
}

static void answer(const char* result)
{
    std::cout << (result ? result : "null") << "\n";
    std::cout.flush();
}

// Checks every module once and exits: the shape a person runs by hand.
static int checkOnce(const std::string& root, const std::vector<std::string>& modules)
{
    // Every module loads before any is checked. A require reaching a
    // module the frontend has not seen resolves to nothing, so checking
    // as we go would make the answer depend on argument order.
    for (const std::string& module : modules)
    {
        std::string source;
        if (!readFile(root + "/" + module, source))
        {
            std::fprintf(stderr, "actias-luau: cannot read %s\n", module.c_str());
            return 2;
        }
        setFile(module.c_str(), source.c_str());
    }

    for (const std::string& module : modules)
        answer(checkScript(module.c_str()));

    return 0;
}

static int serve()
{
    std::string op;
    while (std::cin >> op)
    {
        if (op == "quit")
            break;

        if (op == "set")
        {
            std::size_t pathLen = 0, textLen = 0;
            std::cin >> pathLen >> textLen;
            std::cin.ignore(1); // the newline closing the header
            std::string path, text;
            if (!readExactly(pathLen, path) || !readExactly(textLen, text))
                return 2;
            setFile(path.c_str(), text.c_str());
            continue;
        }

        std::size_t pathLen = 0;
        int line = 0, column = 0;
        const bool positioned =
            op == "complete" || op == "hover" || op == "definition" || op == "signature";

        std::cin >> pathLen;
        if (positioned)
            std::cin >> line >> column;
        std::cin.ignore(1);

        std::string path;
        if (!readExactly(pathLen, path))
            return 2;

        if (op == "remove")
            removeFile(path.c_str());
        else if (op == "check")
            answer(checkScript(path.c_str()));
        else if (op == "semantic")
            answer(semanticScript(path.c_str()));
        else if (op == "complete")
            answer(autocompleteScript(path.c_str(), line, column));
        else if (op == "hover")
            answer(hoverScript(path.c_str(), line, column));
        else if (op == "definition")
            answer(definitionScript(path.c_str(), line, column));
        else if (op == "signature")
            answer(signatureScript(path.c_str(), line, column));
        else
        {
            std::fprintf(stderr, "actias-luau: unknown op '%s'\n", op.c_str());
            return 2;
        }
    }

    return 0;
}

int main(int argc, char** argv)
{
    std::ios::sync_with_stdio(false);

    if (argc == 1)
        return serve();

    if (argc < 3)
    {
        std::fprintf(stderr, "usage: actias-luau [<root> <module>...]\n");
        return 2;
    }

    return checkOnce(argv[1], std::vector<std::string>(argv + 2, argv + argc));
}
