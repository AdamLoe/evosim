#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const scriptDir = path.dirname(new URL(import.meta.url).pathname);
const repoRoot = path.resolve(scriptDir, "..");
const docsRoot = path.join(repoRoot, "docs");
const appRoot = path.join(repoRoot, "app");
const lintedPlan = "docs/plans/orchestrator/07-docs-drift-lint.md";

const failures = [];

function fail(message) {
  failures.push(message);
}

function rel(absPath) {
  return path.relative(repoRoot, absPath).replaceAll(path.sep, "/");
}

function read(relPath) {
  return fs.readFileSync(path.join(repoRoot, relPath), "utf8");
}

function exists(absPath) {
  return fs.existsSync(absPath);
}

function walk(dir, predicate = () => true) {
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.name === "node_modules" || entry.name === "target" || entry.name === ".git") {
      continue;
    }
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...walk(full, predicate));
    } else if (predicate(full)) {
      out.push(full);
    }
  }
  return out.sort();
}

function stripCodeFences(markdown) {
  return markdown.replace(/```[\s\S]*?```/g, "");
}

function targetWithoutFragment(target) {
  const hash = target.indexOf("#");
  return hash >= 0 ? target.slice(0, hash) : target;
}

function isExternalTarget(target) {
  return (
    target === "" ||
    target.startsWith("#") ||
    target.startsWith("~") ||
    /^[a-z][a-z0-9+.-]*:/i.test(target)
  );
}

function resolveLocalPath(fromFile, rawTarget) {
  let target = rawTarget.trim();
  if (target.startsWith("<")) {
    const close = target.indexOf(">");
    target = close >= 0 ? target.slice(1, close) : target.slice(1);
  } else {
    target = target.split(/\s+/)[0];
  }
  target = targetWithoutFragment(target);
  if (isExternalTarget(target)) return null;
  try {
    target = decodeURIComponent(target);
  } catch {
    // Keep the raw target; existence check will report it if invalid.
  }
  return path.resolve(path.dirname(fromFile), target);
}

function checkMarkdownLinks() {
  for (const file of walk(docsRoot, (p) => p.endsWith(".md"))) {
    const relFile = rel(file);
    if (relFile.startsWith("docs/plans/") && relFile !== lintedPlan) continue;
    const markdown = read(rel(file));
    const linkRe = /!?\[[^\]]*]\(([^)]+)\)/g;
    for (const match of markdown.matchAll(linkRe)) {
      const resolved = resolveLocalPath(file, match[1]);
      if (resolved && !exists(resolved)) {
        fail(`${rel(file)} has broken markdown link: ${match[1]}`);
      }
    }
  }
}

function candidatePaths(fromFile, token) {
  const cleaned = targetWithoutFragment(token)
    .replace(/^['"]|['"]$/g, "")
    .replace(/:$/, "");
  if (isExternalTarget(cleaned) || cleaned.includes(" ")) return [];
  const candidates = [];
  if (cleaned.startsWith(".cargo/")) {
    candidates.push(path.join(repoRoot, cleaned));
    candidates.push(path.join(appRoot, cleaned));
  } else if (cleaned.startsWith(".github/")) {
    candidates.push(path.join(repoRoot, cleaned));
  } else if (cleaned.startsWith(".") || cleaned.startsWith("/")) {
    candidates.push(path.resolve(path.dirname(fromFile), cleaned));
  } else {
    candidates.push(path.join(repoRoot, cleaned));
    candidates.push(path.join(appRoot, cleaned));
    candidates.push(path.join(docsRoot, cleaned));
    if (cleaned.startsWith("web/")) {
      candidates.push(path.join(appRoot, cleaned));
    }
    if (cleaned.startsWith("tests/")) {
      candidates.push(path.join(repoRoot, "app/web", cleaned));
    }
  }
  return candidates;
}

function checkInlineCodePaths() {
  const pathLike =
    /^(?:app\/|docs\/|crates\/|web\/|tests\/|\.github\/|\.cargo\/)[A-Za-z0-9_./-]+\.(?:md|rs|ts|tsx|js|mjs|json|toml|yml|yaml|css|html|sh)$/;
  const relativeDocPath = /^\.{1,2}\/[A-Za-z0-9_./-]+\.md$/;
  for (const file of walk(docsRoot, (p) => p.endsWith(".md"))) {
    const relFile = rel(file);
    if (relFile.startsWith("docs/plans/") && relFile !== lintedPlan) continue;
    const markdown = stripCodeFences(read(rel(file)));
    const inlineCodeRe = /`([^`\n]+)`/g;
    for (const match of markdown.matchAll(inlineCodeRe)) {
      const token = match[1].split(/\s+->\s+|\s+→\s+/)[0].trim();
      if (!pathLike.test(token) && !relativeDocPath.test(token)) continue;
      const resolved = candidatePaths(file, token);
      if (!resolved.some(exists)) {
        fail(`${rel(file)} has stale inline path: ${token}`);
      }
    }
  }
}

function parseMarkdownLinks(markdown) {
  const links = [];
  const linkRe = /!?\[[^\]]*]\(([^)]+)\)/g;
  for (const match of markdown.matchAll(linkRe)) {
    let target = match[1].trim();
    if (target.startsWith("<")) {
      const close = target.indexOf(">");
      target = close >= 0 ? target.slice(1, close) : target.slice(1);
    } else {
      target = target.split(/\s+/)[0];
    }
    target = targetWithoutFragment(target);
    if (!isExternalTarget(target)) links.push(target);
  }
  return links;
}

function checkIndexesAndOwnership() {
  for (const indexPath of ["docs/architecture/index.md", "docs/decisions/index.md"]) {
    const indexAbs = path.join(repoRoot, indexPath);
    for (const target of parseMarkdownLinks(read(indexPath))) {
      if (!target.endsWith(".md")) continue;
      const resolved = path.resolve(path.dirname(indexAbs), target);
      if (!exists(resolved)) {
        fail(`${indexPath} routes to missing doc: ${target}`);
      }
    }
  }

  const ownership = JSON.parse(read("docs/_meta/ownership.json"));
  if (!Array.isArray(ownership.concepts)) {
    fail("docs/_meta/ownership.json must contain a concepts array");
    return;
  }
  for (const concept of ownership.concepts) {
    if (!concept.id || !concept.owner) {
      fail("docs/_meta/ownership.json has a concept without id/owner");
      continue;
    }
    const ownerPath = path.join(docsRoot, concept.owner);
    if (!exists(ownerPath)) {
      fail(`ownership concept ${concept.id} owner is missing: ${concept.owner}`);
    }
    for (const refPath of concept.references ?? []) {
      if (refPath.includes("*")) {
        const prefix = refPath.slice(0, refPath.indexOf("*"));
        const dir = path.join(docsRoot, prefix);
        if (!exists(dir)) fail(`ownership concept ${concept.id} has empty glob: ${refPath}`);
      } else if (!exists(path.join(docsRoot, refPath))) {
        fail(`ownership concept ${concept.id} reference is missing: ${refPath}`);
      }
    }
  }
}

function parseTsConstMap(source) {
  const values = new Map();
  const re = /export const ([A-Z0-9_]+) = ([0-9_]+)(?: as const)?;/g;
  for (const match of source.matchAll(re)) {
    values.set(match[1], Number(match[2].replaceAll("_", "")));
  }
  return values;
}

function parseRustConstExpressions(source) {
  const expressions = new Map();
  const re = /pub const ([A-Z0-9_]+): [^=]+ = ([^;]+);/g;
  for (const match of source.matchAll(re)) {
    expressions.set(match[1], match[2].trim());
  }
  return expressions;
}

function evalRustConsts(expressions) {
  const values = new Map();
  let progressed = true;
  while (progressed) {
    progressed = false;
    for (const [name, expr] of expressions) {
      if (values.has(name)) continue;
      let jsExpr = expr
        .replace(/\b\d[\d_]*\b/g, (m) => m.replaceAll("_", ""))
        .replace(/\b([A-Z][A-Z0-9_]*)\b/g, (m) =>
          values.has(m) ? String(values.get(m)) : m,
        );
      if (/[A-Z]/.test(jsExpr)) continue;
      if (!/^[0-9+\-*/ ().]+$/.test(jsExpr)) continue;
      try {
        const value = Function(`"use strict"; return (${jsExpr});`)();
        if (Number.isFinite(value)) {
          values.set(name, value);
          progressed = true;
        }
      } catch {
        // Try again after more names resolve.
      }
    }
  }
  return values;
}

function requireEqual(label, left, right) {
  if (left !== right) {
    fail(`${label} drift: ${left} !== ${right}`);
  }
}

function parseRustSliderNames(source) {
  const match = source.match(/pub const SLIDER_NAMES: &\[&str] = &\[\n([\s\S]*?)\n\];/);
  if (!match) {
    fail("app/crates/evosim/src/wasm_api/mod.rs missing SLIDER_NAMES");
    return [];
  }
  return [...match[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

function parseTsSliderNames(source) {
  const match = source.match(/export const SLIDER_NAMES = \[\n([\s\S]*?)\n] as const;/);
  if (!match) {
    fail("app/web/src/generated/slider-ids.ts missing SLIDER_NAMES");
    return [];
  }
  return [...match[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

function checkConstantsAndMirrors() {
  const controlRust = evalRustConsts(parseRustConstExpressions(read("app/crates/evosim/src/control_sab.rs")));
  const controlTs = parseTsConstMap(read("app/web/src/generated/control-sab.ts"));
  for (const [name, tsValue] of controlTs) {
    if (!controlRust.has(name)) {
      fail(`generated control-sab.ts exports ${name}, but Rust control_sab.rs does not`);
      continue;
    }
    requireEqual(`control SAB ${name}`, controlRust.get(name), tsValue);
  }

  const wasmApiRust = evalRustConsts(parseRustConstExpressions(read("app/crates/evosim/src/wasm_api/mod.rs")));
  const constantsRust = evalRustConsts(parseRustConstExpressions(read("app/crates/evosim/src/constants.rs")));
  const bridgeTs = parseTsConstMap(read("app/web/src/sim/bridge.ts"));
  const lodTs = parseTsConstMap(read("app/web/src/generated/lod-constants.ts"));

  requireEqual(
    "MAX_POP_FOR_SIM Rust/TS",
    constantsRust.get("MAX_POP_FOR_SIM"),
    bridgeTs.get("MAX_POP_FOR_SIM"),
  );
  requireEqual(
    "SNAPSHOT_HEADER_BYTES Rust/TS",
    wasmApiRust.get("SNAPSHOT_HEADER_BYTES"),
    bridgeTs.get("SNAPSHOT_HEADER_BYTES"),
  );
  requireEqual(
    "CREATURE_STRIDE TS vs Rust byte stride",
    wasmApiRust.get("SNAPSHOT_CREATURE_STRIDE") / 4,
    bridgeTs.get("CREATURE_STRIDE"),
  );
  requireEqual(
    "GRASS_LOD_BUDGET_AXIS Rust/generated TS",
    wasmApiRust.get("GRASS_LOD_BUDGET_AXIS"),
    lodTs.get("GRASS_LOD_BUDGET_AXIS"),
  );
  if (!constantsRust.has("NN_INPUTS")) {
    fail("constants.rs missing NN_INPUTS");
  }
  if (!constantsRust.has("STARTING_POP_DEFAULT")) {
    fail("constants.rs missing STARTING_POP_DEFAULT");
  }

  const rustSliders = parseRustSliderNames(read("app/crates/evosim/src/wasm_api/mod.rs"));
  const tsSliders = parseTsSliderNames(read("app/web/src/generated/slider-ids.ts"));
  if (rustSliders.join("\n") !== tsSliders.join("\n")) {
    fail("SLIDER_NAMES drift between wasm_api.rs and generated/slider-ids.ts");
  }
  const sliderTsCount = parseTsConstMap(read("app/web/src/generated/slider-ids.ts")).get("SLIDER_COUNT");
  requireEqual("SLIDER_COUNT generated TS vs Rust names length", rustSliders.length, sliderTsCount);
}

function extractFunctionBody(source, functionName) {
  const start = source.indexOf(`function ${functionName}(`);
  if (start < 0) return "";
  const braceStart = source.indexOf("{", start);
  let depth = 0;
  for (let i = braceStart; i < source.length; i++) {
    if (source[i] === "{") depth++;
    if (source[i] === "}") depth--;
    if (depth === 0) return source.slice(braceStart + 1, i);
  }
  return "";
}

function checkWorkerPacing() {
  const worker = read("app/web/src/sim/worker.ts");
  const loop = extractFunctionBody(worker, "simLoop");
  if (!loop) fail("worker.ts missing simLoop body");
  if (worker.includes("Atomics.waitAsync")) {
    fail("worker.ts uses Atomics.waitAsync; current pacing is synchronous Atomics.wait");
  }
  if (!loop.includes("Atomics.wait(ctrlI32, CTRL_FUTEX, before, Infinity)")) {
    fail("simLoop missing paused Atomics.wait(..., Infinity)");
  }
  if (!loop.includes("Atomics.wait(ctrlI32, CTRL_FUTEX, before, remainingMs)")) {
    fail("simLoop missing target-TPS Atomics.wait(..., remainingMs)");
  }
  if (!loop.includes("remainingMs > 0.25")) {
    fail("simLoop pacing threshold drifted from remainingMs > 0.25");
  }
  if (/\bawait\b|Promise\.resolve|setTimeout/.test(loop)) {
    fail("simLoop should stay synchronous with no await/Promise/setTimeout yield path");
  }
}

function parseTreeOrder(source) {
  const match = source.match(/const TREE_ORDER = \[([^\]]+)]/);
  if (!match) {
    fail("perf-panel.ts missing TREE_ORDER");
    return [];
  }
  return [...match[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

function parseProfilerDocTree(source) {
  const codeBlock = source.match(/```[\s\S]*?```/)?.[0] ?? "";
  const lines = codeBlock.split("\n").map((line) => line.replace(/```/g, ""));
  const topLevel = [];
  const paths = [];
  const stack = [];
  for (const rawLine of lines) {
    if (!rawLine.trim()) continue;
    const indent = rawLine.match(/^ */)?.[0].length ?? 0;
    const text = rawLine.trim().split(/\s+←|\s{2,}/)[0].trim();
    if (!/^[a-z_][a-z0-9_.]*$/.test(text)) continue;
    const depth = Math.floor(indent / 2);
    stack[depth] = text;
    stack.length = depth + 1;
    if (depth === 0) topLevel.push(text);
    paths.push(text);
  }
  return { topLevel, paths };
}

function addPath(set, root, childPath) {
  set.add(root);
  if (childPath) set.add(`${root}.${childPath}`);
}

function checkProfilerDocs() {
  const treeOrder = parseTreeOrder(read("app/web/src/widgets/perf-panel.ts"));
  const profilerDoc = read("docs/architecture/profiler.md");
  const docTree = parseProfilerDocTree(profilerDoc);
  if (docTree.topLevel.join(",") !== treeOrder.join(",")) {
    fail(
      `profiler.md top-level tree drift: docs [${docTree.topLevel.join(", ")}] vs TREE_ORDER [${treeOrder.join(", ")}]`,
    );
  }
  const treeCountRe = new RegExp(`\\*\\*?${treeOrder.length} sibling top-level\\s+trees\\*\\*?`);
  if (!treeCountRe.test(profilerDoc)) {
    fail(`profiler.md should describe ${treeOrder.length} sibling top-level trees`);
  }

  const produced = new Set(treeOrder);
  const sources = [
    read("app/web/src/main.ts"),
    read("app/web/src/render/gl.ts"),
    read("app/web/src/sim/worker.ts"),
    read("app/crates/evosim/src/wasm_api/mod.rs"),
    read("app/crates/evosim/src/world/mod.rs"),
    read("app/crates/evosim/src/world/tick.rs"),
    read("app/crates/evosim/src/world/nn.rs"),
  ].join("\n");

  for (const match of sources.matchAll(/span\("([^"]+)"/g)) {
    produced.add(match[1]);
  }
  for (const match of sources.matchAll(/profile_span!\(&self\.profile,\s*"([^"]+)"/g)) {
    produced.add(match[1]);
  }
  for (const match of sources.matchAll(/record_profile_sample\(\s*"([^"]+)"\s*,\s*"([^"]*)"/gs)) {
    addPath(produced, match[1], match[2]);
  }
  for (const match of sources.matchAll(/record_under_root\(\s*"([^"]+)"\s*,\s*"([^"]*)"/gs)) {
    addPath(produced, match[1], match[2]);
  }
  // The layer rows are generated from format!("forward.l{}", k + 1) for the
  // default 32->48->24->5 topology, so keep the documented current rows covered.
  for (const layer of ["l1", "l2", "l3"]) {
    produced.add(`nn.forward.${layer}`);
  }

  for (const docPath of docTree.paths) {
    if (!produced.has(docPath)) {
      fail(`profiler.md names span ${docPath}, but no producer was found`);
    }
  }
}

function main() {
  checkMarkdownLinks();
  checkInlineCodePaths();
  checkIndexesAndOwnership();
  checkConstantsAndMirrors();
  checkWorkerPacing();
  checkProfilerDocs();

  if (failures.length > 0) {
    console.error("docs-lint failed:");
    for (const failure of failures) {
      console.error(`- ${failure}`);
    }
    process.exit(1);
  }

  console.log("docs-lint passed");
}

main();
