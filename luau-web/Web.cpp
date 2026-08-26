// The workbench's Luau language service: the analysis half of upstream's
// CLI/src/Web.cpp, rebuilt for an editor rather than a demo page.
//
// Differences from upstream, each one a workbench requirement:
//   - a project, not a file: the caller feeds every module through
//     setFile and requires resolve across them, so a symbol defined in
//     lib/domain.lua is typed and jumpable from main.lua;
//   - the default mode is nonstrict like `actias check`, with a file's
//     own --! directive ruling it, so the editor never contradicts the
//     cli;
//   - diagnostics are JSON with begin/end positions and lints included;
//   - autocomplete, hover and go-to-definition exist at all.
//
// Everything is exported as plain C reached through ccall. The caller
// owns diffing: setFile marks a module dirty only when its text
// actually changed, so an edit to one file rechecks one module.

#include "Luau/AstQuery.h"
#include "Luau/Autocomplete.h"
#include "Luau/AutocompleteTypes.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/Frontend.h"
#include "Luau/Scope.h"
#include "Luau/ToString.h"
#include "Luau/TypePack.h"

#include <string>
#include <unordered_map>
#include <unordered_set>

// One in-memory module per project file, keyed by its bundle path.
// `require("lib/domain")` resolves against those keys, with or without
// the `.lua` the file actually carries.
struct BenchFileResolver : Luau::FileResolver
{
    std::optional<Luau::SourceCode> readSource(const Luau::ModuleName& name) override
    {
        auto it = source.find(name);
        if (it == source.end())
            return std::nullopt;

        return Luau::SourceCode{it->second, Luau::SourceCode::Module};
    }

    std::optional<Luau::ModuleName> lookup(std::string name) const
    {
        if (name.rfind("./", 0) == 0)
            name = name.substr(2);
        if (source.count(name))
            return name;
        if (source.count(name + ".lua"))
            return name + ".lua";
        return std::nullopt;
    }

    std::optional<Luau::ModuleInfo> resolveModule(const Luau::ModuleInfo* context, Luau::AstExpr* expr, const Luau::TypeCheckLimits& limits)
        override
    {
        if (Luau::AstExprGlobal* g = expr->as<Luau::AstExprGlobal>())
            return Luau::ModuleInfo{g->name.value};

        if (Luau::AstExprConstantString* str = expr->as<Luau::AstExprConstantString>())
        {
            if (std::optional<Luau::ModuleName> name = lookup(std::string(str->value.data, str->value.size)))
                return Luau::ModuleInfo{*name};
        }

        return std::nullopt;
    }

    std::string getHumanReadableModuleName(const Luau::ModuleName& name) const override
    {
        return name;
    }

    std::optional<std::string> getEnvironmentForModule(const Luau::ModuleName& name) const override
    {
        return std::nullopt;
    }

    std::unordered_map<Luau::ModuleName, std::string> source;
};

// Nonstrict by default, the same default `actias check` runs under; a
// file's own hoisted directive overrides per module.
struct BenchConfigResolver : Luau::ConfigResolver
{
    BenchConfigResolver()
    {
        defaultConfig.mode = Luau::Mode::Nonstrict;
    }

    const Luau::Config& getConfig(const Luau::ModuleName& name, const Luau::TypeCheckLimits& limits) const override
    {
        return defaultConfig;
    }

    Luau::Config defaultConfig;
};

static BenchFileResolver* fileResolver;
static BenchConfigResolver* configResolver;
static Luau::Frontend* frontend;

static void ensureFrontend()
{
    if (frontend)
        return;

    fileResolver = new BenchFileResolver();
    configResolver = new BenchConfigResolver();

    Luau::FrontendOptions options;
    // Hover reads types back out of the checked module, which the
    // frontend discards unless asked to keep whole graphs.
    options.retainFullTypeGraphs = true;
    options.runLintChecks = true;

    frontend = new Luau::Frontend(fileResolver, configResolver, options);
    // The new solver, which is what luau-analyze defaults to (and so
    // what `actias check` actually runs: the cli passes no --solver).
    // It also carries the bidirectional inference that types class-body
    // method parameters from ClassBody's indexer, which the old solver
    // never propagates.
    frontend->setLuauSolverMode(Luau::SolverMode::New);

    Luau::unfreeze(frontend->globals.globalTypes);
    Luau::registerBuiltinGlobals(*frontend, frontend->globals);
    Luau::freeze(frontend->globals.globalTypes);

    // The old solver keeps a second world for autocomplete; without its
    // own builtins (require above all) every completion after a require
    // types as any and lists nothing.
    Luau::unfreeze(frontend->globalsForAutocomplete.globalTypes);
    Luau::registerBuiltinGlobals(*frontend, frontend->globalsForAutocomplete, true);
    Luau::freeze(frontend->globalsForAutocomplete.globalTypes);
}

// The caller diffs before calling, but a redundant setFile must still be
// cheap and must not dirty anything.
extern "C" void setFile(const char* name, const char* text)
{
    ensureFrontend();
    auto it = fileResolver->source.find(name);
    if (it != fileResolver->source.end() && it->second == text)
        return;
    fileResolver->source[name] = text;
    frontend->markDirty(name);
}

extern "C" void removeFile(const char* name)
{
    ensureFrontend();
    if (fileResolver->source.erase(name))
        frontend->markDirty(name);
}

static void appendEscaped(std::string& out, const std::string& text)
{
    for (char c : text)
    {
        switch (c)
        {
        case '"':
            out += "\\\"";
            break;
        case '\\':
            out += "\\\\";
            break;
        case '\n':
            out += "\\n";
            break;
        case '\t':
            out += "\\t";
            break;
        default:
            if ((unsigned char)c < 0x20)
                out += ' ';
            else
                out += c;
        }
    }
}

static void appendLocation(std::string& out, const Luau::Location& location)
{
    out += "\"line\":" + std::to_string(location.begin.line + 1);
    out += ",\"column\":" + std::to_string(location.begin.column + 1);
    out += ",\"endLine\":" + std::to_string(location.end.line + 1);
    out += ",\"endColumn\":" + std::to_string(location.end.column + 1);
}

static void appendDiagnostic(std::string& out, const Luau::Location& location, const char* severity, const std::string& message)
{
    if (out.size() > 1)
        out += ",";
    out += "{";
    appendLocation(out, location);
    out += ",\"severity\":\"";
    out += severity;
    out += "\",\"message\":\"";
    appendEscaped(out, message);
    out += "\"}";
}

// How types read in hovers and completion details: structural for
// anonymous tables (a synthetic name like `visits` says nothing), with
// argument names kept on function signatures.
static Luau::ToStringOptions displayOptions()
{
    Luau::ToStringOptions options;
    options.ignoreSyntheticName = true;
    options.functionTypeArguments = true;
    return options;
}

// Every result is a static string so the pointer survives the return,
// the same caching upstream uses.

extern "C" const char* checkScript(const char* module)
{
    static std::string result;
    result = "[";

    try
    {
        ensureFrontend();
        Luau::CheckResult checkResult = frontend->check(module);

        // The check pulls dependencies in, and their errors carry their
        // own module name; the caller asked about this one.
        for (const Luau::TypeError& error : checkResult.errors)
        {
            if (error.moduleName == module)
                appendDiagnostic(result, error.location, "error", Luau::toString(error));
        }

        for (const Luau::LintWarning& warning : checkResult.lintResult.errors)
            appendDiagnostic(result, warning.location, "error", warning.text);
        for (const Luau::LintWarning& warning : checkResult.lintResult.warnings)
            appendDiagnostic(result, warning.location, "lint", warning.text);
    }
    catch (const std::exception& e)
    {
        result = "[";
        appendDiagnostic(result, Luau::Location(), "error", e.what());
    }

    result += "]";
    return result.c_str();
}

static const char* kindName(Luau::AutocompleteEntryKind kind)
{
    switch (kind)
    {
    case Luau::AutocompleteEntryKind::Property:
        return "property";
    case Luau::AutocompleteEntryKind::Binding:
        return "binding";
    case Luau::AutocompleteEntryKind::Keyword:
        return "keyword";
    case Luau::AutocompleteEntryKind::String:
        return "string";
    case Luau::AutocompleteEntryKind::Type:
        return "type";
    case Luau::AutocompleteEntryKind::Module:
        return "module";
    default:
        return "other";
    }
}

// `line` and `column` are one-based, as the editor counts.
extern "C" const char* autocompleteScript(const char* module, int line, int column)
{
    static std::string result;
    result = "[";

    try
    {
        ensureFrontend();

        Luau::FrontendOptions options = frontend->options;
        options.forAutocomplete = true;
        frontend->check(module, options);

        Luau::Position position(line - 1, column - 1);
        Luau::AutocompleteResult completions = Luau::autocomplete(
            *frontend,
            module,
            position,
            [](const std::string& tag, std::optional<const Luau::ExternType*> ctx,
                std::optional<std::string> contents) -> std::optional<Luau::AutocompleteEntryMap>
            {
                return std::nullopt;
            }
        );

        for (const auto& [name, entry] : completions.entryMap)
        {
            if (result.size() > 1)
                result += ",";
            result += "{\"name\":\"";
            appendEscaped(result, name);
            result += "\",\"kind\":\"";
            result += kindName(entry.kind);
            result += "\"";
            // The access spelling: indexedWithSelf says the caller
            // typed `:`, wrongIndexType says that spelling does not fit
            // this entry, so the editor can put the right one in.
            if (entry.wrongIndexType)
                result += ",\"wrongIndexType\":true";
            if (entry.indexedWithSelf)
                result += ",\"indexedWithSelf\":true";
            if (entry.type)
            {
                Luau::ToStringOptions options = displayOptions();
                std::string type = Luau::toString(*entry.type, options);
                if (type.size() > 200)
                    type = type.substr(0, 200) + "...";
                result += ",\"type\":\"";
                appendEscaped(result, type);
                result += "\"";
            }
            result += "}";
        }
    }
    catch (const std::exception& e)
    {
        result = "[";
    }

    result += "]";
    return result.c_str();
}

namespace
{

// Collects identifier classifications for semantic highlighting. Only
// what the grammar knows for certain is emitted; everything else keeps
// its textmate color. Type indices match the legend the editor
// registers: 0 function, 1 method, 2 property, 3 parameter,
// 4 variable, 5 type.
struct TokenCollector : public Luau::AstVisitor
{
    std::vector<std::array<int, 4>> tokens;
    std::unordered_set<Luau::AstLocal*> parameters;
    std::unordered_set<Luau::AstExpr*> callees;

    void add(const Luau::Location& location, int type)
    {
        if (location.begin.line != location.end.line)
            return;
        tokens.push_back({(int)location.begin.line, (int)location.begin.column,
            (int)(location.end.column - location.begin.column), type});
    }

    bool visit(Luau::AstExprFunction* function) override
    {
        for (Luau::AstLocal* argument : function->args)
        {
            parameters.insert(argument);
            add(argument->location, 3);
        }
        return true;
    }

    bool visit(Luau::AstExprCall* call) override
    {
        callees.insert(call->func);
        if (Luau::AstExprGlobal* global = call->func->as<Luau::AstExprGlobal>())
            add(global->location, 0);
        else if (Luau::AstExprLocal* local = call->func->as<Luau::AstExprLocal>())
            add(local->location, parameters.count(local->local) ? 3 : 0);
        return true;
    }

    bool visit(Luau::AstExprIndexName* index) override
    {
        add(index->indexLocation, callees.count(index) ? 1 : 2);
        return true;
    }

    bool visit(Luau::AstExprLocal* expr) override
    {
        if (!callees.count(expr))
            add(expr->location, parameters.count(expr->local) ? 3 : 4);
        return true;
    }

    bool visit(Luau::AstType* node) override
    {
        return true;
    }

    bool visit(Luau::AstTypeReference* reference) override
    {
        add(reference->nameLocation, 5);
        return true;
    }
};

} // namespace

// Identifier classifications for one module, as [line, column, length,
// type] with zero-based positions.
extern "C" const char* semanticScript(const char* module)
{
    static std::string result;
    result = "[";

    try
    {
        ensureFrontend();
        frontend->check(module);
        const Luau::SourceModule* sourceModule = frontend->getSourceModule(module);
        if (sourceModule && sourceModule->root)
        {
            TokenCollector collector;
            sourceModule->root->visit(&collector);
            for (const auto& token : collector.tokens)
            {
                if (result.size() > 1)
                    result += ",";
                result += "[" + std::to_string(token[0]) + "," + std::to_string(token[1]) + "," +
                          std::to_string(token[2]) + "," + std::to_string(token[3]) + "]";
            }
        }
    }
    catch (const std::exception& e)
    {
        result = "[";
    }

    result += "]";
    return result.c_str();
}

// The type under the cursor, or null when there is nothing to say.
extern "C" const char* hoverScript(const char* module, int line, int column)
{
    static std::string result;
    result.clear();

    try
    {
        ensureFrontend();
        frontend->check(module);

        // One arena under the new solver: the main resolver holds the
        // checked module, and v2 nonstrict keeps real types, so the old
        // strict-autocomplete-arena detour is gone with the old solver.
        const Luau::SourceModule* sourceModule = frontend->getSourceModule(module);
        Luau::ModulePtr checked = frontend->moduleResolver.getModule(module);
        if (!sourceModule || !checked)
            return nullptr;

        Luau::Position position(line - 1, column - 1);
        std::optional<Luau::TypeId> type;
        // A name at its declaration site (a local's, a parameter's) is
        // an AstLocal, not an expression; asked first, so a parameter
        // hover answers with the parameter's type instead of the
        // enclosing function's, which is what the expression query
        // resolves at that position.
        Luau::ExprOrLocal exprOrLocal = Luau::findExprOrLocalAtPosition(*sourceModule, position);
        if (Luau::AstLocal* local = exprOrLocal.getLocal())
        {
            if (Luau::ScopePtr scope = Luau::findScopeAtPosition(*checked, position))
            {
                if (std::optional<Luau::TypeId> found = scope->lookup(local))
                    type = found;
            }
        }
        if (!type)
            type = Luau::findTypeAtPosition(*checked, *sourceModule, position);
        if (!type)
        {
            if (std::optional<Luau::Binding> binding = Luau::findBindingAtPosition(*checked, *sourceModule, position))
                type = binding->typeId;
        }
        if (!type)
            return nullptr;

        Luau::ToStringOptions stringOptions = displayOptions();
        std::string text = Luau::toString(*type, stringOptions);
        if (text.size() > 600)
            text = text.substr(0, 600) + "...";

        result = "{\"type\":\"";
        appendEscaped(result, text);
        result += "\"}";
    }
    catch (const std::exception& e)
    {
        return nullptr;
    }

    return result.empty() ? nullptr : result.c_str();
}

// The enclosing call's signature and which argument the cursor is in,
// or null when the cursor is not inside a call.
extern "C" const char* signatureScript(const char* module, int line, int column)
{
    static std::string result;
    result.clear();

    try
    {
        ensureFrontend();
        frontend->check(module);

        const Luau::SourceModule* sourceModule = frontend->getSourceModule(module);
        Luau::ModulePtr checked = frontend->moduleResolver.getModule(module);
        if (!sourceModule || !checked)
            return nullptr;

        Luau::Position position(line - 1, column - 1);

        // The innermost call whose parentheses hold the cursor.
        Luau::AstExprCall* call = nullptr;
        std::vector<Luau::AstNode*> ancestry = Luau::findAstAncestryOfPosition(*sourceModule, position);
        for (auto it = ancestry.rbegin(); it != ancestry.rend(); ++it)
        {
            if (Luau::AstExprCall* candidate = (*it)->as<Luau::AstExprCall>())
            {
                // The cursor must sit past the callee, or a call inside
                // an argument list would claim its parent's cursor.
                if (position >= candidate->func->location.end)
                {
                    call = candidate;
                    break;
                }
            }
        }
        if (!call)
            return nullptr;

        Luau::TypeId* functionType = checked->astTypes.find(call->func);
        if (!functionType)
            return nullptr;

        Luau::TypeId followed = Luau::follow(*functionType);
        const Luau::FunctionType* fn = Luau::get<Luau::FunctionType>(followed);
        // Overloads and callable handles arrive as intersections; the
        // first function part is the one shown.
        if (!fn)
        {
            if (const Luau::IntersectionType* overloads = Luau::get<Luau::IntersectionType>(followed))
            {
                for (Luau::TypeId part : overloads->parts)
                {
                    if ((fn = Luau::get<Luau::FunctionType>(Luau::follow(part))))
                        break;
                }
            }
        }
        if (!fn)
            return nullptr;

        // Which argument the cursor is in: one past every argument that
        // ends before it.
        size_t active = 0;
        for (size_t i = 0; i < call->args.size; ++i)
        {
            const Luau::Location& argLocation = call->args.data[i]->location;
            if (position > argLocation.end)
                active = i + 1;
            else if (position >= argLocation.begin)
                active = i;
        }

        auto [argTypes, tail] = Luau::flatten(fn->argTypes);
        Luau::ToStringOptions stringOptions = displayOptions();

        // A `:` call consumes the self parameter invisibly; the shown
        // list starts after it.
        size_t first = call->self && !argTypes.empty() ? 1 : 0;

        result = "{\"parameters\":[";
        for (size_t i = first; i < argTypes.size(); ++i)
        {
            if (i > first)
                result += ",";
            std::string label;
            if (i < fn->argNames.size() && fn->argNames[i])
                label = fn->argNames[i]->name + ": ";
            std::string type = Luau::toString(argTypes[i], stringOptions);
            if (type.size() > 80)
                type = type.substr(0, 80) + "...";
            label += type;
            result += "\"";
            appendEscaped(result, label);
            result += "\"";
        }
        result += "],\"active\":" + std::to_string(active);

        auto [returnTypes, returnTail] = Luau::flatten(fn->retTypes);
        if (!returnTypes.empty())
        {
            std::string ret = Luau::toString(returnTypes[0], stringOptions);
            if (ret.size() > 80)
                ret = ret.substr(0, 80) + "...";
            result += ",\"returns\":\"";
            appendEscaped(result, ret);
            result += "\"";
        }
        result += "}";
    }
    catch (const std::exception& e)
    {
        return nullptr;
    }

    return result.empty() ? nullptr : result.c_str();
}

// Where the symbol under the cursor was declared, or null. The target
// carries its module, which may not be the queried one: a property of a
// required module's export jumps into that file.
extern "C" const char* definitionScript(const char* module, int line, int column)
{
    static std::string result;
    result.clear();

    try
    {
        ensureFrontend();
        frontend->check(module);

        // Same resolver as hover, for the same reason: property lookups
        // need the table's real type.
        const Luau::SourceModule* sourceModule = frontend->getSourceModule(module);
        Luau::ModulePtr checked = frontend->moduleResolver.getModule(module);
        if (!sourceModule || !checked)
            return nullptr;

        Luau::Position position(line - 1, column - 1);
        std::optional<Luau::Location> target;
        std::string targetModule = module;

        if (std::optional<Luau::Binding> binding = Luau::findBindingAtPosition(*checked, *sourceModule, position))
            target = binding->location;

        // Property and method names have no binding; the table type
        // remembers where each key was declared, and whose module the
        // table came from.
        if (!target)
        {
            if (Luau::AstExpr* expr = Luau::findExprAtPosition(*sourceModule, position))
            {
                if (Luau::AstExprIndexName* index = expr->as<Luau::AstExprIndexName>())
                {
                    if (Luau::TypeId* tableType = checked->astTypes.find(index->expr))
                    {
                        Luau::TypeId followed = Luau::follow(*tableType);
                        if (const Luau::TableType* table = Luau::get<Luau::TableType>(followed))
                        {
                            auto prop = table->props.find(index->index.value);
                            if (prop != table->props.end() && prop->second.location)
                                target = prop->second.location;
                            // Alias-declared tables (the platform types)
                            // carry no per-key locations; the type's own
                            // definition site is the next best jump.
                            else if (prop != table->props.end())
                                target = table->definitionLocation;

                            if (target && !table->definitionModuleName.empty())
                                targetModule = table->definitionModuleName;
                        }
                    }
                }
            }
        }

        if (!target)
            return nullptr;

        result = "{";
        appendLocation(result, *target);
        result += ",\"module\":\"";
        appendEscaped(result, targetModule);
        result += "\"}";
    }
    catch (const std::exception& e)
    {
        return nullptr;
    }

    return result.empty() ? nullptr : result.c_str();
}
