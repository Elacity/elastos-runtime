import assert from "node:assert/strict";
import test from "node:test";

import {
  browserDisplayMetrics,
} from "./browser-selkies-control-service.mjs";

test("product page raster is the fixed 1080p stream at DPR 1", () => {
  const config = {
    displaySurface: {
      stream: { width: 1920, height: 1080 },
    },
  };
  assert.deepEqual(
    browserDisplayMetrics(config),
    {
      width: 1920,
      height: 1080,
      deviceScaleFactor: 1,
      streamWidth: 1920,
      streamHeight: 1080,
    },
  );
});
