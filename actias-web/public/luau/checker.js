/*
 * The workbench's Luau language service, off the main thread. The wasm
 * module (built from luau-web/, see the README beside this file) loads
 * once and answers requests for the rest of the session.
 *
 * Requests carry the WHOLE project; this worker owns the diff, so the
 * wasm sees setFile only for what changed and an edit to one file
 * dirties one module.
 *
 * Protocol, both directions keyed by `id`:
 *   in   { id, op: 'check',      files, module }
 *        { id, op: 'complete',   files, module, line, column }
 *        { id, op: 'hover',      files, module, line, column }
 *        { id, op: 'definition', files, module, line, column }
 *        { id, op: 'signature',  files, module, line, column }
 *   out  { id, ready: false }        module still loading
 *        { id, result }              parsed JSON, null when nothing
 *        { id, error: string }
 *
 * Positions travel one-based in both directions and refer to the texts
 * as sent; the caller owns any prologue shifting.
 */

let ready = false;
let failed = null;
let parked = [];
const sent = new Map();

// importScripts is synchronous and defines the global Module, but the
// wasm behind it instantiates asynchronously. The callback has to be
// attached AFTER the import: the glue ignores a Module defined
// beforehand, and calling in early throws instead of reporting
// not-ready.
try {
  self.importScripts('/luau/Actias.Luau.js');
  self.Module.onRuntimeInitialized = () => {
    ready = true;
    const queued = parked;
    parked = [];
    queued.forEach(answer);
  };
} catch (error) {
  failed = String(error);
}

function syncFiles(files) {
  for (const [path, text] of Object.entries(files)) {
    if (sent.get(path) === text) continue;
    sent.set(path, text);
    self.Module.ccall('setFile', null, ['string', 'string'], [path, text]);
  }
  for (const path of [...sent.keys()]) {
    if (path in files) continue;
    sent.delete(path);
    self.Module.ccall('removeFile', null, ['string'], [path]);
  }
}

function call(request) {
  syncFiles(request.files);
  switch (request.op) {
    case 'check':
      return self.Module.ccall('checkScript', 'string', ['string'], [request.module]);
    case 'complete':
      return self.Module.ccall(
        'autocompleteScript',
        'string',
        ['string', 'number', 'number'],
        [request.module, request.line, request.column],
      );
    case 'hover':
      return self.Module.ccall(
        'hoverScript',
        'string',
        ['string', 'number', 'number'],
        [request.module, request.line, request.column],
      );
    case 'definition':
      return self.Module.ccall(
        'definitionScript',
        'string',
        ['string', 'number', 'number'],
        [request.module, request.line, request.column],
      );
    case 'signature':
      return self.Module.ccall(
        'signatureScript',
        'string',
        ['string', 'number', 'number'],
        [request.module, request.line, request.column],
      );
    case 'semantic':
      return self.Module.ccall('semanticScript', 'string', ['string'], [
        request.module,
      ]);
    default:
      throw new Error(`unknown op '${request.op}'`);
  }
}

function answer(request) {
  if (failed) {
    self.postMessage({ id: request.id, error: failed });
    return;
  }
  try {
    const raw = call(request);
    self.postMessage({ id: request.id, result: raw ? JSON.parse(raw) : null });
  } catch (error) {
    self.postMessage({ id: request.id, error: String(error) });
  }
}

self.onmessage = (event) => {
  const request = event.data;
  if (!ready && !failed) {
    // Keep the newest of each op: an old check is worthless once a
    // newer one exists, but a queued completion must not evict it.
    parked = parked.filter((waiting) => waiting.op !== request.op);
    parked.push(request);
    self.postMessage({ id: request.id, ready: false });
    return;
  }
  answer(request);
};
