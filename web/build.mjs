// Deploy build. Zero dependencies — run with `node build.mjs` after
// `wasm-pack build bsky-context-web --target web --out-dir ../web/pkg`.
//
// The dev flow is unchanged: serve web/ directly and everything works from
// its original paths (the service worker is skipped on localhost). This
// script exists only to produce the deployable `out/` directory:
//
//   1. Content-hash every asset into its filename and rewrite the references
//      that chain them together:
//        index.html -> app.js -> pkg/bsky_context_web.js -> *_bg.wasm
//   2. Generate sw.js from sw.template.js with the precache manifest spliced
//      in. The SW version is the hash of the manifest, so an unchanged deploy
//      produces a byte-identical sw.js and clients ignore it.
//
// Content-hashing is what makes the service worker's cache-first strategy
// safe: a cached asset is always correct for its URL, and any change shows
// up as a new URL in a new index.html.
//
// Adding an asset: emitHashed() it (with replacements for anything it
// references), rewrite its referrer, and add it to the precache list below.

import { createHash } from "node:crypto";
import {
  copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync,
} from "node:fs";
import { basename, dirname, extname, join } from "node:path";
import { fileURLToPath } from "node:url";

const webDir = dirname(fileURLToPath(import.meta.url));
const outDir = join(webDir, "out");

rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });

function contentHash(buf) {
  return createHash("sha256").update(buf).digest("hex").slice(0, 10);
}

// Copy `srcName` into out/ under a content-hashed name, applying string
// replacements first. Returns the hashed basename for use in referrers.
function emitHashed(srcName, replacements = []) {
  let content = readFileSync(join(webDir, srcName));
  if (replacements.length > 0) {
    let text = content.toString("utf-8");
    for (const [from, to] of replacements) {
      if (!text.includes(from)) {
        throw new Error(`${srcName}: expected reference "${from}" not found`);
      }
      text = text.replaceAll(from, to);
    }
    content = Buffer.from(text, "utf-8");
  }
  const ext = extname(srcName);
  const stem = basename(srcName, ext);
  const name = `${stem}-${contentHash(content)}${ext}`;
  writeFileSync(join(outDir, name), content);
  return name;
}

// 1. Hash the dependency chain bottom-up, rewriting references as we go.
const wasmName = emitHashed("pkg/bsky_context_web_bg.wasm");
const pkgJsName = emitHashed("pkg/bsky_context_web.js", [
  ["bsky_context_web_bg.wasm", wasmName],
]);
const appJsName = emitHashed("app.js", [
  ["./pkg/bsky_context_web.js", `./${pkgJsName}`],
]);
const cssName = emitHashed("styles.css");

// 2. index.html keeps its name; point it at the hashed assets.
let html = readFileSync(join(webDir, "index.html"), "utf-8");
html = html
  .replace('href="styles.css"', `href="${cssName}"`)
  .replace('src="app.js"', `src="${appJsName}"`);
writeFileSync(join(outDir, "index.html"), html);

// 3. Generate the service worker with the precache manifest.
const precache = ["./", "./index.html", appJsName, pkgJsName, wasmName, cssName];
const precacheJson = JSON.stringify(precache);
const swVersion = contentHash(Buffer.from(precacheJson));
const sw = readFileSync(join(webDir, "sw.template.js"), "utf-8")
  .replace("__PRECACHE__", precacheJson)
  .replace("__VERSION__", swVersion);
writeFileSync(join(outDir, "sw.js"), sw);

// 4. Kill-switch travels with every deploy (served as sw-killswitch.js; only
// does anything if manually copied over sw.js to recover bricked clients).
copyFileSync(join(webDir, "sw-killswitch.js"), join(outDir, "sw-killswitch.js"));

console.log(`Built: ${appJsName}, ${pkgJsName}, ${wasmName}, ${cssName}, sw.js (v${swVersion})`);
