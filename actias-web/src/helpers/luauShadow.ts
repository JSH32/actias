import core from '../../../actias-cli/definitions/core.d.luau';
import objects from '../../../actias-cli/definitions/objects.d.luau';
import work from '../../../actias-cli/definitions/work.d.luau';
import http from '../../../actias-cli/definitions/http.d.luau';

/**
 * The platform's declarations, in the order the cli lists them. These
 * are the files the cli ships and luau-lsp reads, imported as source
 * rather than copied, so the three consumers cannot drift apart.
 *
 * The paths are how the workbench presents them: read-only files a
 * definition jump can land in.
 */
export const PLATFORM_DEFINITIONS = [
  { path: 'platform/core.d.luau', text: core },
  { path: 'platform/objects.d.luau', text: objects },
  { path: 'platform/work.d.luau', text: work },
  { path: 'platform/http.d.luau', text: http },
] as const;

/** Where a prologue line came from. `declare` lines were rewritten to
 * `local ...`, which is two columns narrower than the original. */
type Origin = { path: string; line: number; declared: boolean };

/**
 * The declarations as typed local shadows. Each single-line
 * `declare name: T` becomes `local name: T = nil :: any`, everything
 * else passes through, and a trailing keep-alive expression stops the
 * shadows reading as unused.
 *
 * Shadows rather than definition files because the analyzer ignores its
 * definitions flags; the cli's `analyze.rs` does exactly this, and the
 * two must agree or the editor would contradict `actias check`.
 *
 * Every input line yields exactly one output line, so the origin map is
 * line-for-line: origins[i] is where prologue line i+1 came from, null
 * for the header and keep-alive lines that are nobody's.
 */
function buildPrologue(): { text: string; origins: (Origin | null)[] } {
  let out = '-- actias: typed shadows derived from the definitions files\n';
  const origins: (Origin | null)[] = [null];
  const names: string[] = [];

  for (const file of PLATFORM_DEFINITIONS) {
    file.text.split('\n').forEach((line, index) => {
      // `require` stays the real builtin so the analyser resolves
      // modules across the project; a shadow would any-ify every
      // import. The placeholder keeps the origin map line-for-line.
      if (line.startsWith('declare require:')) {
        out += '-- require: the analyser resolves project modules\n';
        origins.push({ path: file.path, line: index + 1, declared: false });
        return;
      }
      if (line.startsWith('declare ')) {
        const rest = line.slice('declare '.length);
        names.push(rest.split(':')[0].trim());
        out += `local ${rest} = nil :: any\n`;
        origins.push({ path: file.path, line: index + 1, declared: true });
      } else {
        out += `${line}\n`;
        origins.push({ path: file.path, line: index + 1, declared: false });
      }
    });
  }

  out += `local _ = ${names.join(' and ')}\n`;
  origins.push(null);
  return { text: out, origins };
}

/** Built once: the definitions are compiled in and never change. */
const { text: PROLOGUE, origins: ORIGINS } = buildPrologue();
const PROLOGUE_LINES = PROLOGUE.split('\n').length - 1;

/**
 * The definitions-file position behind a one-based prologue line, with
 * the column shift a rewritten `declare` line needs ("declare " is two
 * characters wider than "local ").
 */
export function prologueOrigin(
  line: number,
): { path: string; line: number; columnShift: number } | null {
  const origin = ORIGINS[line - 1];
  if (!origin) return null;
  return {
    path: origin.path,
    line: origin.line,
    columnShift: origin.declared ? 2 : 0,
  };
}

/** A source with the platform surface in scope, how far the check's line
 * numbers must be shifted back to point at the user's own code, how many
 * directive lines stayed at the top, and whether the file asked for
 * strict checking. */
export type Shadow = {
  text: string;
  offset: number;
  directives: number;
  strict: boolean;
};

/**
 * Wraps a file so the analyzer sees the platform surface.
 *
 * Luau honours `--!` directives only at the very top of a file, so a
 * file's own directives hoist above the prologue; otherwise a
 * `--!strict` header would silently stop applying.
 */
export function shadow(source: string): Shadow {
  const lines = source.split('\n');
  let index = 0;
  while (index < lines.length && lines[index].trimStart().startsWith('--!')) {
    index += 1;
  }
  const directives = lines.slice(0, index);
  const body = lines.slice(index);

  return {
    text:
      (directives.length ? `${directives.join('\n')}\n` : '') +
      PROLOGUE +
      body.join('\n'),
    // Only the prologue displaces anything. The directives sit at the
    // top in both texts, so they keep their own line numbers and adding
    // them here would shift every diagnostic by one per directive.
    offset: PROLOGUE_LINES,
    directives: directives.length,
    strict: directives.some((line) => line.trimStart().startsWith('--!strict')),
  };
}
