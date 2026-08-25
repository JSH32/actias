/** Luau declaration files are imported as source text (next.config.js
 * gives them webpack's `asset/source` type), so the workbench builds its
 * prologue from the files the cli ships. */
declare module '*.d.luau' {
  const source: string;
  export default source;
}
