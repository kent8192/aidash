import { test } from "node:test";
import assert from "node:assert/strict";
import {
  FAVORITES_LIMIT,
  FAVORITES_TAB,
  OTHER_TAB,
  effortOptions,
  favoritesKey,
  initialTab,
  modelsForTab,
  parseFavorites,
  railTabs,
  readFavorites,
  searchModels,
  toggleFavorite,
  vendorMark,
  vendorOf,
  vendorRail,
  writeFavorites,
} from "../src/model-picker-catalog.ts";

const model = (id, name = id) => ({ id, name });

/** `count` models for each vendor, named `<vendor>/m<i>`. */
function catalog(counts) {
  return Object.entries(counts).flatMap(([vendor, count]) =>
    Array.from({ length: count }, (_, i) => model(`${vendor}/m${i}`)),
  );
}

test("the rail ranks the ten largest vendors by count and puts the rest in Other", () => {
  const models = [
    ...catalog({
      openai: 9,
      qwen: 8,
      google: 7,
      anthropic: 6,
      mistralai: 5,
      "z-ai": 5,
      deepseek: 4,
      nvidia: 3,
      meta: 3,
      "x-ai": 2,
      cohere: 1,
      amazon: 1,
    }),
    model("bare-model"),
  ];
  const rail = vendorRail(models);
  // Ties go by slug, so the order never depends on catalog order.
  assert.deepEqual(rail.vendors, [
    "openai",
    "qwen",
    "google",
    "anthropic",
    "mistralai",
    "z-ai",
    "deepseek",
    "meta",
    "nvidia",
    "x-ai",
  ]);
  assert.equal(rail.other, true);
  assert.deepEqual(railTabs(rail), [FAVORITES_TAB, ...rail.vendors, OTHER_TAB]);
  assert.deepEqual(
    modelsForTab(models, OTHER_TAB, rail, []).map((m) => m.id),
    ["cohere/m0", "amazon/m0", "bare-model"],
  );
  assert.deepEqual(
    modelsForTab(models, "x-ai", rail, []).map((m) => m.id),
    ["x-ai/m0", "x-ai/m1"],
  );
});

test("a vendor named other never shares the Other tab id", () => {
  const models = [model("other/model"), model("vendor/fixture")];
  const rail = vendorRail(models);
  assert.equal(rail.other, false);
  assert.deepEqual(railTabs(rail), [FAVORITES_TAB, "other", "vendor"]);
  assert.deepEqual(
    modelsForTab(models, "other", rail, []).map((m) => m.id),
    ["other/model"],
  );
});

test("vendors come from the slug prefix, with alias tildes folded in", () => {
  assert.equal(vendorOf("anthropic/claude"), "anthropic");
  assert.equal(vendorOf("~openai/gpt-latest"), "openai");
  assert.equal(vendorOf("no-slash"), null);
  assert.equal(vendorOf("/leading"), null);
  assert.equal(vendorMark("~openai"), "O");
  assert.equal(vendorMark("z-ai"), "Z");
});

test("search spans every vendor and matches every word in name or slug", () => {
  const models = [
    model("openai/gpt-5", "GPT-5"),
    model("anthropic/claude-sonnet", "Claude Sonnet"),
    model("google/gemini-pro", "Gemini Pro"),
  ];
  assert.deepEqual(
    searchModels(models, "  ").map((m) => m.id),
    models.map((m) => m.id),
  );
  assert.deepEqual(
    searchModels(models, "CLAUDE").map((m) => m.id),
    ["anthropic/claude-sonnet"],
  );
  assert.deepEqual(
    searchModels(models, "google pro").map((m) => m.id),
    ["google/gemini-pro"],
  );
  assert.deepEqual(searchModels(models, "pro sonnet"), []);
});

test("the picker opens on favorites when one is listed, else the largest vendor", () => {
  const models = catalog({ openai: 2, anthropic: 1 });
  const rail = vendorRail(models);
  assert.equal(initialTab(models, rail, []), "openai");
  // A favorite the catalog no longer lists does not count.
  assert.equal(initialTab(models, rail, ["gone/model"]), "openai");
  assert.equal(initialTab(models, rail, ["anthropic/m0"]), FAVORITES_TAB);
  assert.deepEqual(
    modelsForTab(models, FAVORITES_TAB, rail, ["anthropic/m0", "gone/model"]),
    [model("anthropic/m0")],
  );
});

test("efforts follow the canonical order and only what the catalog supports", () => {
  assert.deepEqual(
    effortOptions({
      supported_efforts: ["max", "low", "unknown", "none", "xhigh", "minimal"],
      mandatory: false,
      default_effort: "low",
    }),
    ["none", "minimal", "low", "xhigh", "max"],
  );
});

test("mandatory reasoning offers no None, and no reasoning offers no efforts", () => {
  assert.deepEqual(
    effortOptions({
      supported_efforts: ["high", "low", "none"],
      mandatory: true,
      default_effort: "low",
    }),
    ["low", "high"],
  );
  assert.deepEqual(effortOptions(null), []);
  assert.deepEqual(effortOptions(undefined), []);
  assert.deepEqual(
    effortOptions({
      supported_efforts: null,
      mandatory: false,
      default_effort: null,
    }),
    [],
  );
});

test("favorites are keyed per Tenant and Subject", () => {
  assert.equal(favoritesKey(), "aidash-model-favorites");
  assert.equal(favoritesKey("acme"), "aidash-model-favorites:acme");
  assert.equal(
    favoritesKey("acme", "user:a@b"),
    "aidash-model-favorites:acme:user%3Aa%40b",
  );
  assert.notEqual(favoritesKey("acme", "a"), favoritesKey("acme", "b"));
});

test("malformed favorites storage reads as an empty or cleaned list", () => {
  assert.deepEqual(parseFavorites(null), []);
  assert.deepEqual(parseFavorites("not json"), []);
  assert.deepEqual(parseFavorites('{"a":1}'), []);
  assert.deepEqual(parseFavorites('"openai/gpt"'), []);
  assert.deepEqual(
    parseFavorites('["openai/gpt", 3, null, "", "openai/gpt", " qwen/q "]'),
    ["openai/gpt", "qwen/q"],
  );
  const many = JSON.stringify(Array.from({ length: 150 }, (_, i) => `v/m${i}`));
  assert.equal(parseFavorites(many).length, FAVORITES_LIMIT);
  const throwing = {
    getItem() {
      throw new Error("denied");
    },
    setItem() {
      throw new Error("denied");
    },
  };
  assert.deepEqual(readFavorites(throwing, "k"), []);
  assert.equal(writeFavorites(throwing, "k", ["a/b"]), false);
  assert.deepEqual(readFavorites(undefined, "k"), []);
});

test("favorites toggle, put the newest first and keep at most 100", () => {
  assert.deepEqual(toggleFavorite([], "a/b"), ["a/b"]);
  assert.deepEqual(toggleFavorite(["a/b", "c/d"], "a/b"), ["c/d"]);
  const full = Array.from({ length: FAVORITES_LIMIT }, (_, i) => `v/m${i}`);
  const next = toggleFavorite(full, "new/model");
  assert.equal(next.length, FAVORITES_LIMIT);
  assert.equal(next[0], "new/model");
  assert.equal(next.includes(`v/m${FAVORITES_LIMIT - 1}`), false);
  const store = new Map();
  const storage = {
    getItem: (key) => store.get(key) ?? null,
    setItem: (key, value) => store.set(key, value),
  };
  assert.equal(writeFavorites(storage, "k", next), true);
  assert.deepEqual(readFavorites(storage, "k"), next);
});
