import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import test from "node:test";

const stageSource = fs.readFileSync(
  new URL("./build/stage-browser-vm-target.sh", import.meta.url),
  "utf8",
);
const patchFunction = stageSource.match(
  /^def patch_selkies_signaling_retries\(source\):\n[\s\S]*?(?=^main_text = patch_selkies_signaling_retries\()/m,
)?.[0];
assert.ok(patchFunction, "stage must apply the extracted signaling retry patch");

// Verbatim handlers from Selkies 1.6.1 __main__.py. Keep upstream indentation
// because the production patch deliberately rejects an unexpected source shape.
const upstreamHandlers = `    # Handle errors from the signalling server
    async def on_signalling_error(e):
       if isinstance(e, WebRTCSignallingErrorNoPeer):
           # Waiting for peer to connect, retry in 2 seconds.
           time.sleep(2)
           await signalling.setup_call()
       else:
           logger.error("signalling error: %s", str(e))
           app.stop_pipeline()
    async def on_audio_signalling_error(e):
       if isinstance(e, WebRTCSignallingErrorNoPeer):
           # Waiting for peer to connect, retry in 2 seconds.
           time.sleep(2)
           await audio_signalling.setup_call()
       else:
           logger.error("signalling error: %s", str(e))
           audio_app.stop_pipeline()
    signalling.on_error = on_signalling_error
    audio_signalling.on_error = on_audio_signalling_error
`;

const pythonPrelude = `
import asyncio
import time
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock

${patchFunction}
source = ${JSON.stringify(upstreamHandlers)}

class WebRTCSignallingErrorNoPeer(Exception):
    pass

signalling = SimpleNamespace(setup_call=AsyncMock())
audio_signalling = SimpleNamespace(setup_call=AsyncMock())
app = SimpleNamespace(stop_pipeline=Mock())
audio_app = SimpleNamespace(stop_pipeline=Mock())
logger = SimpleNamespace(error=Mock())
`;

function runPython(body, timeout = 3000) {
  return spawnSync("python3", ["-u", "-"], {
    input: pythonPrelude + body,
    encoding: "utf8",
    timeout,
    maxBuffer: 16 * 1024,
  });
}

function assertPythonPassed(result) {
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
}

function retryProbe(peer, patched) {
  return `
${patched ? "source = patch_selkies_signaling_retries(source)" : ""}
exec("def bind_handlers():\\n" + source)
bind_handlers()
peer, other_peer, pipeline, other_pipeline = ${peer === "video"
    ? "signalling, audio_signalling, app, audio_app"
    : "audio_signalling, signalling, audio_app, app"}

async def probe():
    real_sleep = asyncio.sleep
    release_retry = asyncio.Event()
    delays = []

    async def controlled_sleep(delay):
        delays.append(delay)
        await release_retry.wait()

    # Only control the retry's clock. The observer still uses actual asyncio
    # scheduling, and the original time.sleep remains blocking in the baseline.
    asyncio.sleep = controlled_sleep
    try:
        task = asyncio.create_task(peer.on_error(WebRTCSignallingErrorNoPeer()))
        print("probe_ready", flush=True)
        await real_sleep(0)
        assert delays == [2], delays
        assert not task.done(), "retry must remain pending while other work runs"
        peer.setup_call.assert_not_awaited()
        other_peer.setup_call.assert_not_awaited()
        pipeline.stop_pipeline.assert_not_called()
        other_pipeline.stop_pipeline.assert_not_called()
        logger.error.assert_not_called()
        print("observer_progress", flush=True)

        release_retry.set()
        await task
        peer.setup_call.assert_awaited_once_with()
        other_peer.setup_call.assert_not_awaited()
        pipeline.stop_pipeline.assert_not_called()
        other_pipeline.stop_pipeline.assert_not_called()

        # A different error still stops only this handler's pipeline, without
        # another retry or delay, and preserves the existing diagnostic.
        await peer.on_error(RuntimeError("fixture signaling failure"))
        pipeline.stop_pipeline.assert_called_once_with()
        other_pipeline.stop_pipeline.assert_not_called()
        peer.setup_call.assert_awaited_once_with()
        other_peer.setup_call.assert_not_awaited()
        assert delays == [2], delays
        logger.error.assert_called_once_with("signalling error: %s", "fixture signaling failure")
    finally:
        asyncio.sleep = real_sleep

asyncio.run(probe())
`;
}

for (const peer of ["video", "audio"]) {
  test(`${peer} upstream retry blocks unrelated work within the subprocess budget`, () => {
    const result = runPython(retryProbe(peer, false), 1000);
    assert.equal(result.error?.code, "ETIMEDOUT", result.stderr);
    assert.match(result.stdout, /^probe_ready$/m, "Python must reach the actual handler probe");
    assert.doesNotMatch(result.stdout, /observer_progress/);
  });

  test(`${peer} patched retry yields, preserves two seconds, and stops its pipeline on other errors`, () => {
    const result = runPython(retryProbe(peer, true));
    assertPythonPassed(result);
    assert.match(result.stdout, /^observer_progress$/m);
  });
}

test("stage retry patch changes only the two waits, is idempotent, and rejects handler drift", () => {
  assertPythonPassed(runPython(`
patched = patch_selkies_signaling_retries(source)
assert patched == source.replace("time.sleep(2)", "await asyncio.sleep(2)")
assert patch_selkies_signaling_retries(patched) == patched
for peer in ("signalling", "audio_signalling"):
    drifted = source.replace("await " + peer + ".setup_call()", "await " + peer + ".changed_call()")
    try:
        patch_selkies_signaling_retries(drifted)
    except SystemExit as error:
        assert "retry patch target not found" in str(error), str(error)
    else:
        raise AssertionError("unexpected handler drift was accepted: " + peer)
`));
});
