const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function waitForReady() {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const pause = document.querySelector("#pause");
    const canvas = document.querySelector("#screen");
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

function renderedCanvas() {
  const canvas = document.querySelector("#screen");
  const blank = document.createElement("canvas");
  blank.width = canvas.width;
  blank.height = canvas.height;
  return canvas.toDataURL() !== blank.toDataURL();
}

async function run() {
  await waitForReady();
  await delay(500);
  const phase = sessionStorage.getItem("gba-linux-proof-phase") || "initial";
  if (phase === "reload") {
    const save = await saveStatus();
    await postResult({
      ok: save.put_count > 0 && save.get_after_put > 0 && save.save_bytes > 0,
      platform: navigator.platform,
      userAgent: navigator.userAgent,
      crossOriginIsolated,
      sharedArrayBuffer: typeof SharedArrayBuffer === "function",
      reloaded: true,
      save,
      renderedAfterReload: renderedCanvas(),
      errors: [],
    });
    return;
  }

  const errors = [];
  window.addEventListener("error", (event) => errors.push(event.message));
  window.addEventListener("unhandledrejection", (event) => errors.push(String(event.reason)));
  const renderedBeforeInput = renderedCanvas();

  const a = document.querySelector('[data-button="a"]');
  window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyX", bubbles: true }));
  await new Promise(requestAnimationFrame);
  const keyboardPressed = a.classList.contains("pressed");
  window.dispatchEvent(new KeyboardEvent("keyup", { code: "KeyX", bubbles: true }));
  await new Promise(requestAnimationFrame);
  const keyboardReleased = !a.classList.contains("pressed");

  const b = document.querySelector('[data-button="b"]');
  const setPointerCapture = b.setPointerCapture;
  b.setPointerCapture = () => {};
  b.dispatchEvent(new PointerEvent("pointerdown", { pointerId: 7, bubbles: true }));
  await new Promise(requestAnimationFrame);
  const controllerPressed = b.classList.contains("pressed");
  b.dispatchEvent(new PointerEvent("pointerup", { pointerId: 7, bubbles: true }));
  await new Promise(requestAnimationFrame);
  const controllerReleased = !b.classList.contains("pressed");
  b.setPointerCapture = setPointerCapture;

  const sound = document.querySelector("#sound");
  const soundDeadline = Date.now() + 10_000;
  while (sound.getAttribute("aria-pressed") !== "true" && Date.now() < soundDeadline) {
    await delay(50);
  }
  const audioEnabled = sound.textContent === "Sound on" && sound.getAttribute("aria-pressed") === "true";

  document.querySelector("#pause").click();
  const deadline = Date.now() + 5_000;
  let save = await saveStatus();
  while (save.put_count === 0 && Date.now() < deadline) {
    await delay(50);
    save = await saveStatus();
  }
  if (
    !renderedBeforeInput ||
    !keyboardPressed ||
    !keyboardReleased ||
    !controllerPressed ||
    !controllerReleased ||
    !audioEnabled ||
    save.put_count === 0 ||
    save.save_bytes === 0 ||
    errors.length
  ) {
    await postResult({
      ok: false,
      renderedBeforeInput,
      keyboardPressed,
      keyboardReleased,
      controllerPressed,
      controllerReleased,
      audioEnabled,
      save,
      errors,
    });
    return;
  }
  sessionStorage.setItem("gba-linux-proof-phase", "reload");
  location.reload();
}

try {
  await run();
} catch (error) {
  await postResult({ ok: false, errors: [String(error?.stack || error)] });
}
