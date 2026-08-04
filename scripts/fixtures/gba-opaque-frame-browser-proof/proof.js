const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function waitForReady() {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const pause = document.querySelector("#btn-pause");
    const canvas = document.querySelector("#canvas");
    if (pause && !pause.disabled && canvas?.width === 240 && canvas?.height === 160) return;
    await delay(50);
  }
  throw new Error(document.querySelector("#status")?.textContent || "GBA did not become ready");
}

async function saveStatus() {
  const response = await fetch("/proof/save-status");
  if (!response.ok) throw new Error("save status unavailable");
  return response.json();
}

async function postResult(result) {
  await fetch("/proof", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(result),
  });
}

async function waitForTrustedInputReceipt() {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    const response = await fetch("/proof/trusted-input-status");
    if (response.ok) {
      const value = await response.json();
      if (
        value.keydown_trusted === true &&
        value.keyup_trusted === true &&
        value.pressed === true &&
        value.released === true &&
        value.start_keydown_trusted === true &&
        value.start_keyup_trusted === true &&
        value.start_pressed === true &&
        value.start_released === true
      ) {
        return value;
      }
    }
    await delay(25);
  }
  throw new Error("trusted input receipt did not arrive");
}

function renderedCanvas() {
  const canvas = document.querySelector("#canvas");
  const blank = document.createElement("canvas");
  blank.width = canvas.width;
  blank.height = canvas.height;
  return canvas.toDataURL() !== blank.toDataURL();
}

function renderProbeSnapshot() {
  const probe = window.__elastosGbaRenderProbe || {};
  return {
    install_calls: Number(probe.install_calls || 0),
    install_context_available: probe.install_context_available === true,
    install_error: String(probe.install_error || ""),
    context_count: Number(probe.context_count || 0),
    context_type: String(probe.context_type || ""),
    put_image_data_calls: Number(probe.put_image_data_calls || 0),
    draw_calls: Number(probe.draw_calls || 0),
    texture_uploads: Number(probe.texture_uploads || 0),
    framebuffer_uploads: Number(probe.framebuffer_uploads || 0),
    changed_framebuffer_uploads: Number(probe.changed_framebuffer_uploads || 0),
    framebuffer_hashes: [...(probe.framebuffer_hashes || [])],
    last_framebuffer_hash: String(probe.last_framebuffer_hash || ""),
    max_framebuffer_bytes: Number(probe.max_framebuffer_bytes || 0),
    nonzero_framebuffer_bytes: Number(probe.nonzero_framebuffer_bytes || 0),
    changed_frame_writes: Number(probe.changed_frame_writes || 0),
    unique_frame_hashes: [...(probe.unique_frame_hashes || [])],
    last_frame_hash: String(probe.last_frame_hash || ""),
    max_image_data_bytes: Number(probe.max_image_data_bytes || 0),
    nonzero_image_data_bytes: Number(probe.nonzero_image_data_bytes || 0),
    last_write_at: Number(probe.last_write_at || 0),
  };
}

function canvasPixelSnapshot() {
  const canvas = document.querySelector("#canvas");
  const blank = document.createElement("canvas");
  blank.width = canvas.width;
  blank.height = canvas.height;
  const frame = canvas.toDataURL();
  let hash = 2166136261;
  for (let index = 0; index < frame.length; index += 1) {
    hash = Math.imul(hash ^ frame.charCodeAt(index), 16777619);
  }
  return {
    hash: (hash >>> 0).toString(16).padStart(8, "0"),
    nonblank: frame !== blank.toDataURL(),
  };
}

async function proveRenderContinuity({ requirePixelChange = true } = {}) {
  // mGBA retains its canvas context. Re-acquiring the same context here lets
  // the product probe wrap the live WebGL/2D object even if module startup won
  // the race with this acceptance observer.
  window.__elastosInstallGbaCanvasProbe?.(document.querySelector("#canvas"));
  const deadline = Date.now() + 10_000;
  const before = renderProbeSnapshot();
  let after = before;
  const pixelHashes = new Set();
  let samples = 0;
  while (Date.now() < deadline) {
    await delay(50);
    const pixels = canvasPixelSnapshot();
    if (pixels.nonblank) pixelHashes.add(pixels.hash);
    samples += 1;
    after = renderProbeSnapshot();
    const actualRenderActivity =
      after.put_image_data_calls > before.put_image_data_calls ||
      after.draw_calls > before.draw_calls ||
      after.texture_uploads > before.texture_uploads;
    const webglActivity = after.framebuffer_uploads > before.framebuffer_uploads &&
      after.nonzero_framebuffer_bytes > 0;
    const webglPixelChange = after.changed_framebuffer_uploads >
      before.changed_framebuffer_uploads && after.framebuffer_hashes.length > 1;
    const twoDimensionalActivity = after.put_image_data_calls >
      before.put_image_data_calls && after.nonzero_image_data_bytes > 0;
    const twoDimensionalPixelChange = after.changed_frame_writes >
      before.changed_frame_writes && after.unique_frame_hashes.length > 1;
    const renderedPath = webglActivity
      ? "webgl-framebuffer-upload"
      : twoDimensionalActivity
        ? "2d-put-image-data"
        : "";
    if (
      actualRenderActivity &&
      Boolean(renderedPath) &&
      samples >= 3 &&
      (!requirePixelChange || webglPixelChange || twoDimensionalPixelChange)
    ) {
      return {
        ...after,
        put_image_data_during_observation:
          after.put_image_data_calls - before.put_image_data_calls,
        draws_during_observation: after.draw_calls - before.draw_calls,
        texture_uploads_during_observation:
          after.texture_uploads - before.texture_uploads,
        pixel_samples: samples,
        canvas_snapshot_hashes: [...pixelHashes],
        renderer_path: renderedPath,
        distinct_pixel_hashes: renderedPath === "2d-put-image-data"
          ? [...after.unique_frame_hashes]
          : [...after.framebuffer_hashes],
        pixel_change_required: requirePixelChange,
      };
    }
  }
  throw new Error(`GBA framebuffer proof did not advance: ${JSON.stringify({
    before,
    after,
    pixel_samples: samples,
    canvas_snapshot_hashes: [...pixelHashes],
    webgl_pixel_hashes: [...after.framebuffer_hashes],
    two_dimensional_pixel_hashes: [...after.unique_frame_hashes],
  })}`);
}

function audioProbeSnapshot() {
  const probe = window.__elastosGbaAudioProbe || {};
  const contexts = probe.contexts || [];
  return {
    context_count: contexts.length,
    running_contexts: contexts.filter((context) => context.state === "running").length,
    script_processor_callbacks: Number(probe.script_processor_callbacks || 0),
    rendered_buffers: Number(probe.rendered_buffers || 0),
    samples_examined: Number(probe.samples_examined || 0),
    nonzero_samples: Number(probe.nonzero_samples || 0),
    max_abs_sample: Number(probe.max_abs_sample || 0),
  };
}

async function proveAudioOutput() {
  const deadline = Date.now() + 10_000;
  let proof = audioProbeSnapshot();
  while (Date.now() < deadline) {
    if (
      proof.running_contexts > 0 &&
      proof.script_processor_callbacks > 0 &&
      proof.rendered_buffers > 0 &&
      proof.samples_examined > 0 &&
      proof.nonzero_samples > 0 &&
      proof.max_abs_sample > 0
    ) {
      return proof;
    }
    await delay(50);
    proof = audioProbeSnapshot();
  }
  throw new Error(`GBA produced no observable non-zero audio output: ${JSON.stringify(proof)}`);
}

async function run() {
  await waitForReady();
  let persisted = {};
  try {
    persisted = JSON.parse(window.name || "{}");
  } catch {}
  const phase = persisted?.schema === "elastos.gba.opaque-frame-proof/v2"
    ? persisted.phase
    : "initial";
  const initialRenderActivityPromise = phase === "initial"
    ? proveRenderContinuity()
    : null;
  if (phase === "initial") parent.postMessage({ type: "gba-proof-ready" }, "*");
  const trustedInput = await waitForTrustedInputReceipt();
  if (phase === "reload") {
    const slotDeadline = Date.now() + 10_000;
    while (document.querySelector("#slot-status1")?.textContent !== "Saved" && Date.now() < slotDeadline) {
      await delay(50);
    }
    document.querySelector("#btn-load1")?.click();
    const loadDeadline = Date.now() + 5_000;
    while (document.querySelector("#status")?.textContent !== "State 1 loaded" && Date.now() < loadDeadline) {
      await delay(50);
    }
    const save = await saveStatus();
    const initial = persisted.initial || {};
    const initialPassed = [
      "rendered",
      "renderContinuity",
      "keyboardPressed",
      "keyboardReleased",
      "trustedStart",
      "controllerPressed",
      "controllerReleased",
      "audioRendered",
      "stateSaved",
      "stateLoaded",
    ].every((name) => initial[name] === true);
    const stateLoadedAfterReload = document.querySelector("#status")?.textContent === "State 1 loaded";
    const renderActivityAfterReload = await proveRenderContinuity({ requirePixelChange: false });
    const renderedAfterReload = renderedCanvas();
    window.name = "";
    await postResult({
      ok:
        save.put_count > 0 &&
        save.get_after_put > 0 &&
        save.save_bytes > 0 &&
        save.state_put_count > 0 &&
        save.state_get_after_put > 0 &&
        save.state_bytes > 0 &&
        stateLoadedAfterReload &&
        renderedAfterReload &&
        initialPassed,
      platform: navigator.platform,
      userAgent: navigator.userAgent,
      crossOriginIsolated,
      sharedArrayBuffer: typeof SharedArrayBuffer === "function",
      reloaded: true,
      save,
      initial,
      stateLoadedAfterReload,
      renderedAfterReload,
      renderActivityAfterReload,
      errors: [],
    });
    return;
  }

  const errors = [];
  window.addEventListener("error", (event) => errors.push(event.message));
  window.addEventListener("unhandledrejection", (event) => errors.push(String(event.reason)));
  const rendered = renderedCanvas();
  const renderActivity = await initialRenderActivityPromise;

  const keyboardPressed = trustedInput.keydown_trusted && trustedInput.pressed;
  const keyboardReleased = trustedInput.keyup_trusted && trustedInput.released;
  const trustedStart = trustedInput.start_keydown_trusted &&
    trustedInput.start_keyup_trusted &&
    trustedInput.start_pressed &&
    trustedInput.start_released;

  const b = document.querySelector('[data-key="b"]');
  const setPointerCapture = b.setPointerCapture;
  b.setPointerCapture = () => {};
  b.dispatchEvent(new PointerEvent("pointerdown", { pointerId: 7, bubbles: true }));
  await new Promise(requestAnimationFrame);
  const controllerPressed = b.classList.contains("pressed");
  b.dispatchEvent(new PointerEvent("pointerup", { pointerId: 7, bubbles: true }));
  await new Promise(requestAnimationFrame);
  const controllerReleased = !b.classList.contains("pressed");
  b.setPointerCapture = setPointerCapture;

  const audioOutput = await proveAudioOutput();
  const audioRendered = audioOutput.nonzero_samples > 0;

  document.querySelector("#btn-save1").click();
  const stateDeadline = Date.now() + 5_000;
  let save = await saveStatus();
  while (save.state_put_count === 0 && Date.now() < stateDeadline) {
    await delay(50);
    save = await saveStatus();
  }
  const stateSaved = save.state_put_count > 0 && document.querySelector("#slot-status1")?.textContent === "Saved";
  document.querySelector("#btn-load1").click();
  const stateLoadDeadline = Date.now() + 5_000;
  while (document.querySelector("#status")?.textContent !== "State 1 loaded" && Date.now() < stateLoadDeadline) {
    await delay(50);
  }
  const stateLoaded = document.querySelector("#status")?.textContent === "State 1 loaded";

  document.querySelector("#btn-pause").click();
  const deadline = Date.now() + 5_000;
  save = await saveStatus();
  while (save.put_count === 0 && Date.now() < deadline) {
    await delay(50);
    save = await saveStatus();
  }
  if (
    !rendered ||
    (renderActivity.put_image_data_during_observation < 1 &&
      renderActivity.draws_during_observation < 1 &&
      renderActivity.texture_uploads_during_observation < 1) ||
    renderActivity.distinct_pixel_hashes.length < 2 ||
    (renderActivity.nonzero_framebuffer_bytes < 1 &&
      renderActivity.nonzero_image_data_bytes < 1) ||
    !keyboardPressed ||
    !keyboardReleased ||
    !trustedStart ||
    !controllerPressed ||
    !controllerReleased ||
    !audioRendered ||
    !stateSaved ||
    !stateLoaded ||
    save.put_count === 0 ||
    save.save_bytes === 0 ||
    errors.length
  ) {
    await postResult({
      ok: false,
      rendered,
      renderActivity,
      keyboardPressed,
      keyboardReleased,
      trustedStart,
      controllerPressed,
      controllerReleased,
      audioRendered,
      audioOutput,
      stateSaved,
      stateLoaded,
      save,
      errors,
    });
    return;
  }
  window.name = JSON.stringify({
    schema: "elastos.gba.opaque-frame-proof/v2",
    phase: "reload",
    initial: {
      rendered,
      renderContinuity: renderActivity.distinct_pixel_hashes.length > 1 && (
        renderActivity.put_image_data_during_observation > 0 ||
        renderActivity.draws_during_observation > 0 ||
        renderActivity.texture_uploads_during_observation > 0
      ),
      renderActivity,
      keyboardPressed,
      keyboardReleased,
      trustedStart,
      controllerPressed,
      controllerReleased,
      audioRendered,
      audioOutput,
      stateSaved,
      stateLoaded,
    },
  });
  location.reload();
}

try {
  await run();
} catch (error) {
  await postResult({ ok: false, errors: [String(error?.stack || error)] });
}
