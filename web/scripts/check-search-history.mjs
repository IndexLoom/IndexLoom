import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

// Test the production TypeScript module without adding a separate test framework.
const tsSource = await readFile(new URL("../src/lib/search-history.ts", import.meta.url), "utf8");
const js = ts.transpileModule(tsSource, { compilerOptions: {
  module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022,
} }).outputText;
const { recordSearch, readSearchHistory, deleteSearchHistory } =
  await import("data:text/javascript;base64," + Buffer.from(js).toString("base64"));

const values = new Map();
globalThis.crypto ??= { randomUUID };
globalThis.localStorage = {
  getItem: (key) => values.get(key) ?? null,
  setItem: (key, value) => { values.set(key, value); },
  removeItem: (key) => { values.delete(key); },
  clear: () => { values.clear(); },
};

function query(name) {
  return { query: name, resultCount: 2, indexerCount: 3, language: "ru" };
}
test("browser history is isolated by authenticated account identity", () => {
  values.clear();
  recordSearch("user:alice-123", query("private alice"));
  recordSearch("user:bob-456", query("private bob"));
  assert.deepEqual(readSearchHistory("user:alice-123").map((entry) => entry.query), ["private alice"]);
  assert.deepEqual(readSearchHistory("user:bob-456").map((entry) => entry.query), ["private bob"]);
  assert.deepEqual(readSearchHistory("operator"), []);
  assert.deepEqual(readSearchHistory("anonymous"), []);
  assert.equal(values.size, 2);
});

test("deleting one account's history does not affect another", () => {
  values.clear();
  recordSearch("user:alice-123", query("alpha"));
  recordSearch("user:bob-456", query("beta"));
  deleteSearchHistory("user:alice-123");
  assert.deepEqual(readSearchHistory("user:alice-123"), []);
  assert.equal(readSearchHistory("user:bob-456")[0].query, "beta");
});

test("shared legacy browser history is purged, not imported into a new account", () => {
  values.clear();
  values.set("indexloom.search-history.v1", JSON.stringify([{ query: "previous account" }]));
  assert.deepEqual(readSearchHistory("user:new"), []);
  assert.equal(values.has("indexloom.search-history.v1"), false);
});

test("missing or invalid identity cannot save or read search history", () => {
  values.clear();
  for (const scope of ["", "user:", "user:../other", "other"]) {
    recordSearch(scope, query("sensitive query"));
    assert.deepEqual(readSearchHistory(scope), []);
  }
  assert.equal(values.size, 0);
});

test("record cap and invalid entries are enforced within each account", () => {
  values.clear();
  for (let i = 0; i < 110; i++) recordSearch("operator", query("q" + i));
  assert.equal(readSearchHistory("operator").length, 100);
  recordSearch("operator", query("last item"));
  assert.equal(readSearchHistory("operator")[0].query, "last item");
  recordSearch("operator", query("x".repeat(501)));
  assert.equal(readSearchHistory("operator").length, 100);
  assert.equal(readSearchHistory("user:safe").length, 0);
});
