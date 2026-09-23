#!/usr/bin/env node
// One version source: the Cargo workspace version. Fails (exit 1) if
// package.json, package-lock.json or tauri.conf.json drift from it. The UI chip
// reads the version from the core at runtime (BuildInfo), so it cannot drift.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (p) => readFileSync(join(root, p), "utf8");

const cargo = read("Cargo.toml");
const ws = cargo.split(/^\[workspace\.package\]/m)[1];
const m = ws && ws.match(/^version\s*=\s*"([^"]+)"/m);
if (!m) {
  console.error("check-version: no [workspace.package] version in Cargo.toml");
  process.exit(1);
}
const expected = m[1];

const found = {
  "package.json": JSON.parse(read("package.json")).version,
  "package-lock.json": JSON.parse(read("package-lock.json")).version,
  "src-tauri/tauri.conf.json": JSON.parse(read("src-tauri/tauri.conf.json")).version,
};

// Every crate must inherit the workspace version.
for (const dir of ["crates/contract", "crates/prompt", "crates/llm", "crates/stt", "crates/audio", "crates/settings", "crates/session", "crates/shell", "src-tauri"]) {
  const toml = read(`${dir}/Cargo.toml`);
  found[`${dir}/Cargo.toml`] = /^version\.workspace\s*=\s*true/m.test(toml) ? expected : "(not inherited)";
}

let bad = 0;
for (const [file, v] of Object.entries(found)) {
  if (v !== expected) {
    console.error(`check-version: ${file} has ${v}, expected ${expected}`);
    bad++;
  }
}
if (bad) process.exit(1);
console.log(`check-version: all ${Object.keys(found).length} sources at ${expected}`);
