import init, { crawl_web, render_web, lensNames, parse_post_ref, webId } from "./pkg/bsky_context_web.js";

const el = (id) => document.getElementById(id);
const form = el("crawl-form");
const status = el("status");
const output = el("output");
const lensSelect = el("lens");
const lensParams = el("lens-params");

const LENS_PARAMS = {
  threads: [["top", "number", "Top"]],
  highlights: [["top", "number", "Top"]],
  neighborhood: [["hops", "number", "Hops"], ["uri", "text", "Post URI"]],
  timeline: [["after", "text", "After (ISO)"], ["before", "text", "Before (ISO)"]],
  search: [["query", "text", "Query"], ["author", "text", "Author"]],
};

let webJson = null;

function setStatus(text, isError = false) {
  status.textContent = text;
  status.classList.toggle("error", isError);
}

function readOptions() {
  const num = (id) => {
    const v = el(id).value.trim();
    return v === "" ? undefined : Number(v);
  };
  return {
    max_nodes: num("opt-max-nodes"),
    max_depth: num("opt-max-depth"),
    timeout_secs: num("opt-timeout"),
    concurrency: num("opt-concurrency"),
  };
}

function currentParams() {
  const params = {};
  for (const input of lensParams.querySelectorAll("input")) {
    const v = input.value.trim();
    if (v === "") continue;
    params[input.dataset.param] = input.type === "number" ? Number(v) : v;
  }
  return params;
}

function rerender() {
  if (webJson === null) return;
  try {
    output.textContent = render_web(webJson, lensSelect.value, JSON.stringify(currentParams()));
  } catch (err) {
    output.textContent = `Render failed: ${err}`;
  }
}

function buildLensParams() {
  lensParams.replaceChildren();
  for (const [name, type, label] of LENS_PARAMS[lensSelect.value] ?? []) {
    const wrapper = document.createElement("label");
    wrapper.textContent = `${label} `;
    const input = document.createElement("input");
    input.type = type;
    input.dataset.param = name;
    input.size = type === "text" ? 18 : 4;
    input.addEventListener("input", rerender);
    wrapper.append(input);
    lensParams.append(wrapper);
  }
}

async function runCrawl(event) {
  event.preventDefault();
  const button = el("crawl-btn");
  let uri;
  try {
    uri = parse_post_ref(el("post-url").value);
  } catch (err) {
    setStatus(String(err), true);
    return;
  }
  button.disabled = true;
  const started = performance.now();
  setStatus("Crawling...");
  try {
    const resultJson = await crawl_web(uri, JSON.stringify(readOptions()), null, (p) => {
      const secs = Math.round((performance.now() - started) / 1000);
      setStatus(`Crawling... ${p.nodes} posts, ${p.threads} threads, ${p.edges} edges (${secs}s)`);
    });
    const result = JSON.parse(resultJson);
    webJson = JSON.stringify(result.web);
    const meta = result.web.meta;
    const secs = ((performance.now() - started) / 1000).toFixed(1);
    let note = `Done in ${secs}s: ${meta.node_count} posts, ${meta.thread_count} threads, ${meta.edge_count} edges.`;
    if (result.stop_reason !== "complete") {
      note += ` Stopped at ${result.stop_reason.replace("_", " ")}; ${result.pending} threads unexplored.`;
    }
    setStatus(note);
    el("result").hidden = false;
    rerender();
    history.replaceState(null, "", `#${encodeURIComponent(uri)}`);
  } catch (err) {
    setStatus(`Crawl failed: ${err}`, true);
  } finally {
    button.disabled = false;
  }
}

function download() {
  if (webJson === null) return;
  const root = JSON.parse(webJson).meta.root_uri;
  const blob = new Blob([webJson], { type: "application/json" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = `${webId(root)}.json`;
  a.click();
  URL.revokeObjectURL(a.href);
}

// Offline support. Skipped on localhost so dev never fights a stale cache;
// persistent storage keeps the browser from evicting the cache under pressure.
function registerServiceWorker() {
  if (!("serviceWorker" in navigator)) return;
  if (location.hostname === "localhost" || location.hostname === "127.0.0.1") return;
  navigator.serviceWorker.register("./sw.js").catch((err) => {
    console.warn("Service worker registration failed:", err);
  });
  if (navigator.storage?.persist) {
    navigator.storage.persist().catch(() => {});
  }
}

await init();

for (const name of lensNames()) {
  const option = document.createElement("option");
  option.value = name;
  option.textContent = name;
  lensSelect.append(option);
}
lensSelect.addEventListener("change", () => { buildLensParams(); rerender(); });
buildLensParams();
form.addEventListener("submit", runCrawl);
el("copy-btn").addEventListener("click", () => navigator.clipboard.writeText(output.textContent));
el("download-btn").addEventListener("click", download);

if (location.hash.length > 1) {
  el("post-url").value = decodeURIComponent(location.hash.slice(1));
}

registerServiceWorker();
