import assert from "node:assert/strict";
import test from "node:test";
import { abandonable } from "../src/collaboration/workspace-model.ts";

test("failed, blocked and cancelled tasks can be abandoned whatever ran", () => {
  for (const status of ["FAILED", "BLOCKED", "CANCELLED"]) {
    assert.equal(abandonable({ status }, []), true, status);
    assert.equal(abandonable({ status }, [{}]), true, status);
  }
});

test("an open task nobody has run can be abandoned", () => {
  assert.equal(abandonable({ status: "OPEN" }, []), true);
});

test("an open task with a Run is left to the server, which refuses a Run under way", () => {
  assert.equal(abandonable({ status: "OPEN" }, [{}]), false);
});

test("claimed, running and finished tasks cannot be abandoned", () => {
  for (const status of ["CLAIMED", "RUNNING", "COMPLETED", "ABANDONED"]) {
    assert.equal(abandonable({ status }, []), false, status);
  }
});
