#!/usr/bin/env node
// Compile-checks the Windows C# sources on any OS (Linux, macOS, Windows) — no Windows SDK, .NET
// SDK or Mono needed. It compiles them exactly as the Windows build does with .NET Framework's
// csc.exe: C# 5, against the .NET Framework 4.8 reference assemblies, warning level 4.
//
//   node packaging/windows/csharp-check/check.cjs            # compile both, run the logic tests
//   node packaging/windows/csharp-check/check.cjs --no-setup  # skip the Setup EXE
//   node packaging/windows/csharp-check/check.cjs --no-tests  # compile only
//
// It also runs the registry cleanup code of PreviewHandler.cs against an in-memory registry
// (tests/). Exits 0 when everything compiles with no warnings and every test passes, 1 otherwise,
// 2 when the toolchain could not be set up. See README.md in this folder for how it works.
'use strict';

const { execFileSync } = require('child_process');
const fs = require('fs');
const http = require('http');
const path = require('path');

const HERE = __dirname;
const REPO = path.resolve(HERE, '..', '..', '..');
const CACHE = path.join(HERE, '.cache');
const BLAZOR_WASM = '@live-codes/blazor-wasm@0.1.1';
const JSDOM = 'jsdom@24.1.3';
const REFASM_REPO = 'https://github.com/mono/reference-assemblies';
const REFASM_COMMIT = '02349172cfa5732cdf2c6c5aa5d1dd2315213b70';
const FRAMEWORK = path.join(CACHE, 'node_modules', '@live-codes', 'blazor-wasm');
const REFS = path.join(CACHE, 'reference-assemblies', 'v4.8');

const FRAMEWORK_REFS = ['mscorlib.dll', 'System.dll', 'System.Core.dll', 'System.Drawing.dll', 'System.Windows.Forms.dll'];
const JOBS = [
  {
    title: 'packaging/windows/PreviewHandler.cs -> LinkcoPdfPreviewHandler.dll',
    name: 'LinkcoPdfPreviewHandler',
    kind: 'library',
    refs: FRAMEWORK_REFS,
    sources: () => [['PreviewHandler.cs', fs.readFileSync(path.join(REPO, 'packaging', 'windows', 'PreviewHandler.cs'))]],
  },
  {
    title: 'installer.py (embedded C#) -> LinkcoPDFEditorSetup.exe',
    name: 'LinkcoPDFEditorSetup',
    kind: 'winexe',
    refs: FRAMEWORK_REFS.concat(['System.IO.Compression.dll', 'System.IO.Compression.FileSystem.dll']),
    setup: true,
    sources: () => {
      const py = [
        'import importlib.util, sys',
        'sys.path.insert(0, ".")',
        'spec = importlib.util.spec_from_file_location("installer", "installer.py")',
        'm = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)',
        'src = m.CSHARP_INSTALLER_SOURCE.replace("__FILE_VERSION__", "0.0.0.0").replace("__DISPLAY_VERSION__", "0.0.0")',
        'sys.stdout.buffer.write(src.encode("utf-8"))',
      ].join('\n');
      const python = process.platform === 'win32' ? 'python' : 'python3';
      return [['Setup.cs', execFileSync(python, ['-c', py], { cwd: REPO, maxBuffer: 64 << 20 })]];
    },
  },
];

function run(cmd, args, opts) {
  execFileSync(cmd, args, Object.assign({ stdio: ['ignore', 'ignore', 'inherit'], shell: process.platform === 'win32' && cmd === 'npm' }, opts));
}

function ensureToolchain() {
  fs.mkdirSync(CACHE, { recursive: true });
  if (!fs.existsSync(path.join(FRAMEWORK, '_framework', 'blazor.webassembly.js')) || !fs.existsSync(path.join(CACHE, 'node_modules', 'jsdom'))) {
    console.error(`==> Fetching ${BLAZOR_WASM} (.NET WebAssembly runtime + Roslyn) and ${JSDOM} from npm...`);
    run('npm', ['install', '--no-save', '--no-package-lock', '--no-audit', '--no-fund', '--prefix', CACHE, BLAZOR_WASM, JSDOM]);
  }
  if (!fs.existsSync(path.join(REFS, 'System.Windows.Forms.dll'))) {
    console.error(`==> Fetching the .NET Framework 4.8 reference assemblies (${REFASM_REPO} @ ${REFASM_COMMIT.slice(0, 12)})...`);
    const dir = path.dirname(REFS);
    fs.rmSync(dir, { recursive: true, force: true });
    fs.mkdirSync(dir, { recursive: true });
    const git = (...a) => run('git', ['-C', dir, ...a]);
    git('init', '-q');
    git('remote', 'add', 'origin', REFASM_REPO);
    git('sparse-checkout', 'set', 'v4.8');
    git('fetch', '-q', '--depth', '1', '--filter=blob:none', 'origin', REFASM_COMMIT);
    git('checkout', '-q', 'FETCH_HEAD');
  }
}

/**
 * The registry code under test, cut out of PreviewHandler.cs: the RegRoot class and these members
 * of LinkcoPdfPreviewHandler / LinkcoPdfThumbnailProvider (all overloads). Compiled with
 * tests/FakeRegistry.cs in place of Microsoft.Win32's registry and PreviewEnvironment.
 */
const TESTED_METHODS = ['UnregisterFromRoot', 'RestoreOrRemoveShellEx', 'RestoreOrRemove', 'DeleteIfEmpty', 'ClsidExists',
  'ReadUserChoiceProgId', 'ProgIdsPointingAtUs', 'HasPerUserRegistration', 'CleanUserHive', 'CleanUserRoot',
  'PreviouslyTouchedProgIds', 'AddUnique', 'UnregisterThumbnailProvider', 'ProgIdShellExPath', 'ReadClassesRootDefault',
  'BackupAndSetShellEx'];
const TESTED_FIELDS = ['ClsidBraced', 'PreviewHandlerCategoryGuid', 'LinkcoConfigKey', 'HandlerProgId', 'EdgePreviewHandlerClsid',
  'OwnProgIds', 'KnownPdfProgIds', 'KnownSharedProgIds', 'SysPdfThumbnailPath'];
const TESTED_THUMBNAIL_CONSTS = ['ClsidBraced', 'ThumbnailCategoryGuid', 'ProgIdName'];

/** The text from `start` through the brace that closes the first `{` after it. */
function braceBlock(src, start) {
  let depth = 0;
  for (let i = src.indexOf('{', start); i >= 0 && i < src.length; i++) {
    if (src[i] === '{') depth++;
    else if (src[i] === '}' && --depth === 0) return src.slice(start, i + 1);
  }
  throw new Error(`unbalanced braces after offset ${start}`);
}

function extractTestedCode(src) {
  const need = (cond, what) => {
    if (!cond) throw new Error(`PreviewHandler.cs: ${what} not found (update csharp-check/check.cjs if it was renamed)`);
  };
  const rr = src.indexOf('    internal sealed class RegRoot');
  need(rr >= 0, 'class RegRoot');
  const methods = TESTED_METHODS.map((name) => {
    const re = new RegExp(`\\n        (?:public|private|internal) static [\\w<>\\[\\]]+ ${name}\\(`, 'g');
    const found = [...src.matchAll(re)].map((m) => braceBlock(src, m.index + 1));
    need(found.length > 0, `method ${name}`);
    return found.join('\n');
  });
  const fields = TESTED_FIELDS.map((name) => {
    const m = new RegExp(`\\n        (?:public|private|internal) (?:const|static readonly) [\\w\\[\\]]+ ${name} = `).exec(src);
    need(m, `field ${name}`);
    return src.slice(m.index + 1, src.indexOf(';', m.index) + 1);
  });
  const thumbAt = src.indexOf('class LinkcoPdfThumbnailProvider');
  need(thumbAt >= 0, 'class LinkcoPdfThumbnailProvider');
  const thumbConsts = TESTED_THUMBNAIL_CONSTS.map((name) => {
    const m = new RegExp(`(?:public|internal|private) const string ${name} = [^;]+;`).exec(src.slice(thumbAt));
    need(m, `LinkcoPdfThumbnailProvider.${name}`);
    return '        ' + m[0];
  });
  return [
    'namespace LinkcoPdfPreview {',
    'using System; using System.Collections.Generic; using System.IO; using Microsoft.Win32;',
    braceBlock(src, rr),
    'internal static class LinkcoPdfThumbnailProvider {', ...thumbConsts, '}',
    'internal static partial class LinkcoPdfPreviewHandler {', ...fields, ...methods, '}',
    '}',
  ].join('\n');
}

/** Serves the WebAssembly bundle to the runtime, which loads its assemblies with fetch(). */
function serve(root) {
  const types = { '.js': 'text/javascript', '.wasm': 'application/wasm', '.json': 'application/json' };
  const server = http.createServer((req, res) => {
    const file = path.join(root, path.normalize(decodeURIComponent(req.url.split('?')[0])).replace(/^(\.\.[/\\])+/, ''));
    fs.readFile(file, (err, data) => {
      if (err) {
        res.writeHead(404);
        res.end();
        return;
      }
      res.writeHead(200, { 'content-type': types[path.extname(file)] || 'application/octet-stream' });
      res.end(data);
    });
  });
  return new Promise((resolve) => server.listen(0, '127.0.0.1', () => resolve(server)));
}

/** Boots the .NET runtime + BlazorRunner in Node, with jsdom providing the DOM Blazor expects. */
async function bootRunner(base) {
  const { JSDOM: Dom } = require(path.join(CACHE, 'node_modules', 'jsdom'));
  const dom = new Dom(`<!doctype html><html><head><base href="${base}"></head><body><div id="blazor-app"></div></body></html>`, { url: base, pretendToBeVisual: true });
  const w = dom.window;
  globalThis.window = globalThis;
  for (const k of ['document', 'location', 'history', 'HTMLElement', 'Element', 'Node', 'Text', 'Comment', 'DocumentFragment', 'MutationObserver', 'Event', 'CustomEvent', 'KeyboardEvent', 'MouseEvent', 'HTMLInputElement', 'HTMLSelectElement', 'HTMLTextAreaElement', 'HTMLScriptElement', 'HTMLLinkElement', 'HTMLTemplateElement', 'HTMLFormElement', 'HTMLAnchorElement', 'SVGElement', 'DOMParser', 'Range', 'NodeFilter', 'customElements', 'localStorage', 'sessionStorage', 'getComputedStyle', 'requestAnimationFrame', 'cancelAnimationFrame', 'matchMedia']) {
    const v = w[k];
    try {
      Object.defineProperty(globalThis, k, { value: typeof v === 'function' && /^[a-z]/.test(k) ? v.bind(w) : v, configurable: true, writable: true });
    } catch (e) {
      // Read-only Node globals (e.g. navigator) are fine as they are.
    }
  }
  globalThis.addEventListener = w.addEventListener.bind(w);
  globalThis.removeEventListener = w.removeEventListener.bind(w);
  globalThis.dispatchEvent = w.dispatchEvent.bind(w);
  const nodeFetch = globalThis.fetch;
  globalThis.fetch = (resource, init) => {
    const url = new URL(typeof resource === 'string' ? resource : resource.url || resource.href, base).href;
    return nodeFetch(url, Object.assign({}, init, { credentials: undefined }));
  };

  const fwDir = path.join(FRAMEWORK, '_framework');
  require(path.join(fwDir, 'blazor.webassembly.js'));
  await globalThis.Blazor.start({
    // dotnet.js is an ES module: Node imports it from disk; everything else is fetched over HTTP.
    loadBootResource: (type, name) => (type === 'dotnetjs' ? require('url').pathToFileURL(path.join(fwDir, name)).href : `${base}_framework/${name}`),
  });
  const invoke = async (method, ...args) => {
    for (let attempt = 0; ; attempt++) {
      try {
        return await globalThis.DotNet.invokeMethodAsync('BlazorRunner', method, ...args);
      } catch (e) {
        if (String((e && e.message) || e).includes('no loaded assembly') && attempt < 240) {
          await new Promise((r) => setTimeout(r, 250));
          continue;
        }
        throw e;
      }
    }
  };
  await invoke('SetBaseUrl', base);
  return invoke;
}

async function main() {
  const skipSetup = process.argv.includes('--no-setup');
  try {
    ensureToolchain();
  } catch (e) {
    console.error(`csharp-check: could not set up the toolchain: ${e.message}`);
    return 2;
  }
  // The bundle's dotnet.js is an ES module without "type": "module"; Node says so once per run.
  process.removeAllListeners('warning');

  const driver = fs.readFileSync(path.join(HERE, 'roslyn-driver.cs'), 'utf8');
  const server = await serve(FRAMEWORK);
  let failed = false;
  try {
    const invoke = await bootRunner(`http://127.0.0.1:${server.address().port}/`);
    for (const job of JOBS) {
      if (job.setup && skipSetup) continue;
      let stdin = `kind=${job.kind}\nlang=5\nname=${job.name}\n`;
      for (const r of job.refs) stdin += `ref=${r}\ndata:${r}=${fs.readFileSync(path.join(REFS, r)).toString('base64')}\n`;
      for (const [name, bytes] of job.sources()) stdin += `src=${name}\ndata:${name}=${Buffer.from(bytes).toString('base64')}\n`;
      const result = JSON.parse(await invoke('RunCode', driver, stdin));
      const out = result.output || '';
      const summary = /RESULT success=(\w+) errors=(\d+) warnings=(\d+)/.exec(out);
      const ok = result.success && summary && summary[1] === 'True' && summary[3] === '0';
      process.stdout.write(out.replace(/^RESULT .*$/m, '').trimEnd() + (out.trim().startsWith('RESULT') ? '' : '\n'));
      for (const e of result.errors || []) console.log(`driver error ${e.id || ''}: ${e.message}`);
      console.log(`${ok ? 'ok  ' : 'FAIL'} ${job.title}${summary ? ` (${summary[2]} errors, ${summary[3]} warnings)` : ''}`);
      failed = failed || !ok;
    }
    if (!process.argv.includes('--no-tests')) {
      const src = fs.readFileSync(path.join(REPO, 'packaging', 'windows', 'PreviewHandler.cs'), 'utf8');
      const program = [
        fs.readFileSync(path.join(HERE, 'tests', 'FakeRegistry.cs'), 'utf8'),
        extractTestedCode(src),
        fs.readFileSync(path.join(HERE, 'tests', 'RegistryCleanupTests.cs'), 'utf8'),
      ].join('\n');
      const result = JSON.parse(await invoke('RunCode', program, ''));
      const out = (result.output || '').trimEnd();
      const ok = result.success && /^ALL PASS$/m.test(out);
      if (!ok || process.argv.includes('--verbose')) console.log(out);
      for (const e of result.errors || []) console.log(`test error ${e.id || ''}: ${e.message}`);
      const checks = (out.match(/^ {2}ok {3}/gm) || []).length;
      console.log(`${ok ? 'ok  ' : 'FAIL'} PreviewHandler.cs registry cleanup tests (${checks} checks passed)`);
      failed = failed || !ok;
    }
  } catch (e) {
    console.error(`csharp-check: ${(e && e.stack) || e}`);
    return 2;
  } finally {
    server.close();
  }
  return failed ? 1 : 0;
}

main().then((code) => process.exit(code));
