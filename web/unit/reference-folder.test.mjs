import { test } from "node:test";
import assert from "node:assert/strict";
import {
  batches,
  folderFiles,
  mergeReferences,
  pagesWithoutTextNotice,
} from "../src/capabilities/reference-folder-model.ts";

const offered = (relativePath, size = 10) => ({
  relativePath,
  size,
  file: relativePath,
});

test("a folder offers its documents by their path inside it", () => {
  assert.deepEqual(
    folderFiles([offered("docs/backend/api.md"), offered("docs/a.md")]).map(
      (file) => file.path,
    ),
    ["a.md", "backend/api.md"],
  );
});

test("hidden files, hidden folders and empty files are never offered", () => {
  assert.deepEqual(
    folderFiles([
      offered("docs/.git/config"),
      offered("docs/.gitignore"),
      offered("docs/keep/.gitkeep", 0),
      offered("docs/empty.md", 0),
      offered("docs/kept.md"),
    ]).map((file) => file.path),
    ["kept.md"],
  );
});

test("requests stay under the limit and a large file goes alone", () => {
  const files = [{ size: 6 }, { size: 6 }, { size: 20 }, { size: 1 }];
  assert.deepEqual(
    batches(files, 12).map((batch) => batch.map((file) => file.size)),
    [[6, 6], [20], [1]],
  );
});

test("pages an Agent cannot read are named, and a scan says so", () => {
  assert.equal(pagesWithoutTextNotice([], 12, false), undefined);
  assert.equal(
    pagesWithoutTextNotice([3, 5], 12, true),
    "12 ページ中 3, 5 ページは文字が読めません。",
  );
  assert.match(pagesWithoutTextNotice([1, 2], 2, false), /scanned PDF/);
});

test("many small files still split into requests the server accepts", () => {
  const files = Array.from({ length: 5 }, () => ({ size: 1 }));
  assert.deepEqual(
    batches(files, 100, 2).map((batch) => batch.length),
    [2, 2, 1],
  );
});

test("a Source never holds a Reference or a path twice", () => {
  const entry = (reference_id, path) => ({ reference_id, path });
  assert.deepEqual(
    mergeReferences(
      [entry("a", "a.md"), entry("b", "b.md")],
      [entry("a", "moved/a.md"), entry("c", "b.md")],
    ),
    [entry("a", "moved/a.md"), entry("c", "b.md")],
  );
});
