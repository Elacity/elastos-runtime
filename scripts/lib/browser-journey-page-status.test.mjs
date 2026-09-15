import assert from "node:assert/strict";
import test from "node:test";
import {
  readBrowserPageStatus,
} from "./browser-journey-page-status.mjs";

const lastPageStatus = {
  schema: "elastos.browser.page-status/v1",
  page_id: "page-current",
  actual_url: "http://localhost/nav",
};
const freshPageStatus = {
  ...lastPageStatus,
  actual_url: "http://localhost/nav?fresh=1",
};

function httpResponse(status, body) {
  return {
    ok: status >= 200 && status < 300,
    status,
    async json() {
      if (body instanceof Error) throw body;
      return body;
    },
  };
}

test("a fresh Engine status replaces lastPageStatus", async () => {
  const result = await readBrowserPageStatus({
    fetchImpl: async () => httpResponse(200, freshPageStatus),
    url: "http://runtime/status",
    lastPageStatus,
    budgetMs: 800,
  });
  assert.equal(result.page_status_fresh, true);
  assert.equal(result.page_status.actual_url, freshPageStatus.actual_url);
});

for (const status of [403, 404, 500]) {
  test(`HTTP ${status} after restore stays an error`, async () => {
    await assert.rejects(readBrowserPageStatus({
      fetchImpl: async () => httpResponse(status, { error: "denied" }),
      url: "http://runtime/status",
      lastPageStatus,
      budgetMs: 800,
    }), error => {
      assert.equal(error.name, "PageStatusHttpError");
      assert.equal(error.status, status);
      return true;
    });
  });
}

test("invalid JSON after restore stays an error", async () => {
  await assert.rejects(readBrowserPageStatus({
    fetchImpl: async () => httpResponse(200, new SyntaxError("bad json")),
    url: "http://runtime/status",
    lastPageStatus,
    budgetMs: 800,
  }), error => {
    assert.equal(error.name, "PageStatusJsonError");
    return true;
  });
});

test("an 800 ms abort reuses lastPageStatus and marks it stale", async () => {
  const result = await readBrowserPageStatus({
    fetchImpl: (_url, options) => new Promise((_, reject) => {
      options.signal.addEventListener("abort", () => {
        const error = new Error("The operation was aborted");
        error.name = "AbortError";
        reject(error);
      }, { once: true });
    }),
    url: "http://runtime/status",
    lastPageStatus,
    budgetMs: 20,
  });
  assert.equal(result.page_status_fresh, false);
  assert.equal(result.page_status, lastPageStatus);
});

test("a timeout without lastPageStatus stays an abort", async () => {
  await assert.rejects(readBrowserPageStatus({
    fetchImpl: (_url, options) => new Promise((_, reject) => {
      options.signal.addEventListener("abort", () => {
        const error = new Error("The operation was aborted");
        error.name = "AbortError";
        reject(error);
      }, { once: true });
    }),
    url: "http://runtime/status",
    lastPageStatus: null,
    budgetMs: 20,
  }), error => {
    assert.equal(error.name, "AbortError");
    return true;
  });
});
