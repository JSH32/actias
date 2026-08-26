/**
 * The workbench's bundle vocabulary: the config face, the seed tree,
 * what counts as text, the base64 files travel in, and the explorer's
 * row order.
 */
export const CONFIG_FILE = 'script.json';

export const DEFAULT_CONFIG = JSON.stringify(
  { entryPoint: 'main.lua', includes: ['**/*.lua'], ignore: [] },
  null,
  2,
);

/** What the workbench can hold and the live session should serve:
 * everything text. Binary assets stay behind publish, since a decoded
 * byte blob does not survive a text editor. */
export const isTextAsset = (path: string) =>
  /\.(lua|luau|json|html|css|js|sql|md|txt)$/.test(path);

export const DEFAULT_FILES: Record<string, string> = {
  [CONFIG_FILE]: DEFAULT_CONFIG,
  'main.lua': `-- Served live at the session url; publish when it feels right.
local visits = kv "workbench"

on "fetch" (function(request)
    local seen = (visits:get("count") or 0) + 1
    visits:set("count", seen)
    return {
        body = json.stringify({ hello = "workbench", visits = seen }),
        headers = { ["Content-Type"] = "application/json" },
    }
end)
`,
};

/** utf-8 safe base64, the encoding bundle files travel in. */
export const encode = (source: string) =>
  btoa(unescape(encodeURIComponent(source)));
export const decode = (content: string) =>
  decodeURIComponent(escape(atob(content)));

/** The explorer's rows: directories once, then their files. */
export function treeEntries(
  paths: string[],
): { kind: 'dir' | 'file'; path: string }[] {
  const rows: { kind: 'dir' | 'file'; path: string }[] = [];
  const seenDirs = new Set<string>();
  for (const path of paths) {
    const parts = path.split('/');
    for (let depth = 1; depth < parts.length; depth += 1) {
      const dir = parts.slice(0, depth).join('/');
      if (!seenDirs.has(dir)) {
        seenDirs.add(dir);
        rows.push({ kind: 'dir', path: dir });
      }
    }
    rows.push({ kind: 'file', path });
  }
  return rows;
}
