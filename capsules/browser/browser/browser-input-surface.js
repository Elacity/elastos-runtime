const WHEEL_FLUSH_MS = 80;
const MOUSE_DRAG_THRESHOLD_PX = 4;

export function plainTextFromClipboardEvent(event) {
  const clipboardData = event?.clipboardData;
  if (!clipboardData || typeof clipboardData.getData !== "function") {
    return "";
  }
  const types = Array.from(clipboardData.types || []);
  if (!types.includes("text/plain")) {
    return "";
  }
  const text = clipboardData.getData("text/plain");
  return typeof text === "string" ? text : "";
}

export function bindBrowserInputSurface({
  copyRemoteClipboardToHost,
  friendlyOpenError,
  getCurrentPage,
  getInputOwner,
  sameInputOwner,
  getCurrentView,
  keyboardCapture,
  pasteHostClipboardIntoRemote,
  readHostClipboardText,
  remoteVideo,
  renderPanel,
  releaseDragButton,
  sendBrowserInput,
  showStatus,
  streamsPointerDrag = () => false,
  unlockRemoteAudioFromGesture,
  requestAnimationFrameFn = globalThis.requestAnimationFrame?.bind(globalThis),
  cancelAnimationFrameFn = globalThis.cancelAnimationFrame?.bind(globalThis),
}) {
  let wheelTimer = 0;
  let wheelDelta = { x: 0, y: 0 };
  let touchPanState = null;
  let mouseDragState = null;
  let suppressSyntheticClickUntil = 0;
  const hostModifierState = {
    control: false,
    meta: false,
  };

  function focusKeyboardCapture() {
    const target = keyboardCapture || renderPanel;
    target.focus({ preventScroll: true });
    if (keyboardCapture) {
      keyboardCapture.value = "";
      keyboardCapture.setSelectionRange?.(0, 0);
    }
  }

  function isAddressInputTarget(target) {
    return Boolean(
      target &&
        (target.closest?.("#browser-form") ||
          target.closest?.("#browser-settings-panel") ||
          target.id === "browser-settings" ||
          target.id === "browser-exit" ||
          target.classList?.contains("browser-exit-select") ||
          target.id === "browser-url" ||
          target.classList?.contains("browser-address")),
    );
  }

  function updateHostModifierState(event, pressed) {
    if (event.key === "Control" || event.code === "ControlLeft" || event.code === "ControlRight") {
      hostModifierState.control = pressed;
    }
    if (event.key === "Meta" || event.code === "MetaLeft" || event.code === "MetaRight") {
      hostModifierState.meta = pressed;
    }
  }

  function isPasteChord(event) {
    if (event.altKey) {
      return false;
    }
    const modifier =
      event.ctrlKey ||
      event.metaKey ||
      event.getModifierState?.("Control") ||
      event.getModifierState?.("Meta") ||
      hostModifierState.control ||
      hostModifierState.meta;
    return Boolean(
      modifier && (event.key?.toLowerCase?.() === "v" || event.code === "KeyV"),
    );
  }

  function pasteFromHostClipboard() {
    readHostClipboardText()
      .then((text) => pasteHostClipboardIntoRemote(text))
      .catch(() => {
        showStatus("Clipboard paste was cancelled or unavailable.", {
          sticky: true,
        });
      });
  }

  function handlePasteChord(event) {
    if (!getCurrentPage() || isAddressInputTarget(event.target) || !isPasteChord(event)) {
      return false;
    }
    unlockRemoteAudioFromGesture();
    event.preventDefault();
    event.stopPropagation();
    pasteFromHostClipboard();
    return true;
  }

  function browserPointFromEvent(event, clampToMedia = false) {
    const target = remoteVideo;
    if (!target || target.hidden) {
      return null;
    }
    const rect = target.getBoundingClientRect();
    const view = getCurrentView() || {};
    const width = Number(target.videoWidth || view.width || rect.width);
    const height = Number(target.videoHeight || view.height || rect.height);
    const contentRect = browserMediaContentRect(target, width, height);
    if (!contentRect) {
      return null;
    }
    const x = ((event.clientX - contentRect.left) / contentRect.width) * width;
    const y = ((event.clientY - contentRect.top) / contentRect.height) * height;
    if (!clampToMedia && (x < 0 || y < 0 || x > width || y > height)) {
      return null;
    }
    return {
      x: Math.max(0, Math.min(width, x)),
      y: Math.max(0, Math.min(height, y)),
    };
  }

  function browserMediaContentRect(target, mediaWidth, mediaHeight) {
    const rect = target.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      return null;
    }
    const objectFit =
      target.ownerDocument?.defaultView?.getComputedStyle?.(target)?.objectFit ||
      "";
    if (objectFit === "fill") {
      return {
        left: rect.left,
        top: rect.top,
        width: rect.width,
        height: rect.height,
      };
    }
    if (
      !Number.isFinite(mediaWidth) ||
      !Number.isFinite(mediaHeight) ||
      mediaWidth <= 0 ||
      mediaHeight <= 0
    ) {
      return {
        left: rect.left,
        top: rect.top,
        width: rect.width,
        height: rect.height,
      };
    }
    const elementRatio = rect.width / rect.height;
    const mediaRatio = mediaWidth / mediaHeight;
    if (Math.abs(elementRatio - mediaRatio) < 0.001) {
      return {
        left: rect.left,
        top: rect.top,
        width: rect.width,
        height: rect.height,
      };
    }
    if (elementRatio > mediaRatio) {
      const height = rect.height;
      const width = height * mediaRatio;
      return {
        left: rect.left + (rect.width - width) / 2,
        top: rect.top,
        width,
        height,
      };
    }
    const width = rect.width;
    const height = width / mediaRatio;
    return {
      left: rect.left,
      top: rect.top + (rect.height - height) / 2,
      width,
      height,
    };
  }

  function sendClickFromEvent(event) {
    unlockRemoteAudioFromGesture();
    const point = browserPointFromEvent(event);
    if (!point) {
      return;
    }
    event.preventDefault();
    focusKeyboardCapture();
    sendBrowserInput(
      { type: "click", x: point.x, y: point.y },
      { history: "push" },
    ).catch((error) => {
      showStatus(friendlyOpenError(error), { sticky: true });
    });
  }

  function isMediaClickTarget(target) {
    return Boolean(
      target &&
        remoteVideo &&
        (target === remoteVideo || remoteVideo.contains?.(target)),
    );
  }

  function queueWheelInput(point, deltaX, deltaY) {
    wheelDelta.x += deltaX;
    wheelDelta.y += deltaY;
    window.clearTimeout(wheelTimer);
    wheelTimer = window.setTimeout(() => {
      const delta = wheelDelta;
      wheelDelta = { x: 0, y: 0 };
      sendBrowserInput(
        {
          type: "wheel",
          x: point?.x,
          y: point?.y,
          delta_x: delta.x,
          delta_y: delta.y,
        },
        { history: "replace" },
      ).catch((error) =>
        showStatus(friendlyOpenError(error), { sticky: true }),
      );
    }, WHEEL_FLUSH_MS);
  }

  remoteVideo.addEventListener("click", (event) => {
    if (Date.now() < suppressSyntheticClickUntil) {
      event.preventDefault();
      return;
    }
    event.stopPropagation();
    sendClickFromEvent(event);
  });

  renderPanel.addEventListener("click", (event) => {
    if (Date.now() < suppressSyntheticClickUntil) {
      event.preventDefault();
      return;
    }
    if (isMediaClickTarget(event.target)) {
      return;
    }
    sendClickFromEvent(event);
  });

  renderPanel.addEventListener("pointerdown", unlockRemoteAudioFromGesture, {
    passive: true,
  });
  renderPanel.addEventListener(
    "pointerdown",
    (event) => {
      if (event.pointerType !== "touch" && event.pointerType !== "pen") {
        return;
      }
      if (!getCurrentPage()) {
        return;
      }
      event.preventDefault();
      focusKeyboardCapture();
      touchPanState = {
        pointerId: event.pointerId,
        x: event.clientX,
        y: event.clientY,
        moved: false,
      };
      renderPanel.setPointerCapture?.(event.pointerId);
    },
    { passive: false },
  );

  renderPanel.addEventListener(
    "pointermove",
    (event) => {
      if (
        !touchPanState ||
        event.pointerId !== touchPanState.pointerId ||
        !getCurrentPage()
      ) {
        return;
      }
      event.preventDefault();
      const dx = touchPanState.x - event.clientX;
      const dy = touchPanState.y - event.clientY;
      if (Math.abs(dx) < 1 && Math.abs(dy) < 1) {
        return;
      }
      touchPanState.x = event.clientX;
      touchPanState.y = event.clientY;
      touchPanState.moved = true;
      queueWheelInput(browserPointFromEvent(event), dx, dy);
    },
    { passive: false },
  );

  renderPanel.addEventListener(
    "pointerup",
    (event) => {
      if (!touchPanState || event.pointerId !== touchPanState.pointerId) {
        return;
      }
      event.preventDefault();
      renderPanel.releasePointerCapture?.(event.pointerId);
      const moved = touchPanState.moved;
      touchPanState = null;
      suppressSyntheticClickUntil = Date.now() + 700;
      if (!moved) {
        sendClickFromEvent(event);
      }
    },
    { passive: false },
  );

  renderPanel.addEventListener("pointercancel", (event) => {
    if (touchPanState?.pointerId === event.pointerId) {
      touchPanState = null;
    }
  });

  function sendMouseMove(state, point) {
    sendBrowserInput(
      { type: "mouse", x: point.x, y: point.y, buttons: 1 },
      // The release goes straight to the channel; moves of an ended drag
      // still waiting in the queue must not press the button again.
      { history: "replace", isStale: () => state.ended },
    ).catch((error) => {
      showStatus(friendlyOpenError(error), { sticky: true });
    });
  }

  // A drag belongs to the page it started on. Once that page is replaced the
  // drag ends; nothing more is sent for it to the replacement.
  function dragOwnerChanged(state) {
    return !sameInputOwner(getInputOwner(), state.owner);
  }

  function flushMouseDragMove() {
    if (!mouseDragState) {
      return;
    }
    mouseDragState.frame = 0;
    if (dragOwnerChanged(mouseDragState)) {
      finishMouseDrag(null);
      return;
    }
    const point = mouseDragState.pendingPoint;
    mouseDragState.pendingPoint = null;
    if (point) {
      mouseDragState.lastPoint = point;
      sendMouseMove(mouseDragState, point);
    }
  }

  function finishMouseDrag(event) {
    const state = mouseDragState;
    mouseDragState = null;
    if (!state?.dragging) {
      return false;
    }
    state.ended = true;
    if (state.frame) {
      cancelAnimationFrameFn?.(state.frame);
    }
    if (renderPanel.hasPointerCapture?.(state.pointerId)) {
      renderPanel.releasePointerCapture?.(state.pointerId);
    }
    const point =
      (event && browserPointFromEvent(event, true)) ||
      state.pendingPoint ||
      state.lastPoint;
    try {
      releaseDragButton({ type: "mouse", x: point.x, y: point.y, buttons: 0 }, state.owner);
    } catch (error) {
      showStatus(friendlyOpenError(error), { sticky: true });
    }
    suppressSyntheticClickUntil = Date.now() + 700;
    return true;
  }

  // A mouse press without movement stays the Runtime click (its completion
  // orders following text). Movement past the threshold becomes a held-button
  // drag over the Selkies datachannel, so the guest can select text.
  renderPanel.addEventListener("pointerdown", (event) => {
    if (event.pointerType === "mouse") {
      // A drag suppresses only its own trailing click, not the next press.
      suppressSyntheticClickUntil = 0;
    }
    if (
      event.pointerType !== "mouse" ||
      event.button !== 0 ||
      !getCurrentPage() ||
      !streamsPointerDrag()
    ) {
      return;
    }
    const start = browserPointFromEvent(event);
    if (!start) {
      return;
    }
    mouseDragState = {
      pointerId: event.pointerId,
      owner: getInputOwner(),
      clientX: event.clientX,
      clientY: event.clientY,
      start,
      lastPoint: start,
      pendingPoint: null,
      dragging: false,
      ended: false,
      frame: 0,
    };
  });

  renderPanel.addEventListener("pointermove", (event) => {
    const state = mouseDragState;
    if (!state || event.pointerId !== state.pointerId) {
      return;
    }
    if (dragOwnerChanged(state)) {
      finishMouseDrag(null);
      return;
    }
    if (!state.dragging) {
      const dx = event.clientX - state.clientX;
      const dy = event.clientY - state.clientY;
      if (Math.hypot(dx, dy) < MOUSE_DRAG_THRESHOLD_PX) {
        return;
      }
      state.dragging = true;
      unlockRemoteAudioFromGesture();
      focusKeyboardCapture();
      renderPanel.setPointerCapture?.(event.pointerId);
      sendMouseMove(state, state.start);
    }
    event.preventDefault();
    // Leaving the video still extends the selection to its edge.
    const point = browserPointFromEvent(event, true);
    if (!point) {
      return;
    }
    state.pendingPoint = point;
    if (!state.frame) {
      state.frame = requestAnimationFrameFn
        ? requestAnimationFrameFn(flushMouseDragMove)
        : (flushMouseDragMove(), 0);
    }
  });

  renderPanel.addEventListener("pointerup", (event) => {
    if (mouseDragState?.pointerId !== event.pointerId) {
      return;
    }
    if (finishMouseDrag(event)) {
      event.preventDefault();
    }
  });

  for (const type of ["pointercancel", "lostpointercapture"]) {
    renderPanel.addEventListener(type, (event) => {
      if (mouseDragState?.pointerId === event.pointerId) {
        finishMouseDrag(null);
      }
    });
  }
  // Losing the window or tab mid-drag must still release the guest button.
  window.addEventListener("blur", () => finishMouseDrag(null));
  window.addEventListener("pagehide", () => finishMouseDrag(null));
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") {
      finishMouseDrag(null);
    }
  });

  renderPanel.addEventListener(
    "wheel",
    (event) => {
      if (!getCurrentPage()) {
        return;
      }
      unlockRemoteAudioFromGesture();
      event.preventDefault();
      focusKeyboardCapture();
      const point = browserPointFromEvent(event);
      queueWheelInput(point, event.deltaX, event.deltaY);
    },
    { passive: false },
  );

  renderPanel.addEventListener("keydown", (event) => {
    if (!getCurrentPage()) {
      return;
    }
    unlockRemoteAudioFromGesture();
    updateHostModifierState(event, true);
    if (handlePasteChord(event)) {
      return;
    }
    if (
      (event.ctrlKey || event.metaKey) &&
      !event.altKey &&
      event.key.toLowerCase() === "c"
    ) {
      event.preventDefault();
      copyRemoteClipboardToHost().catch((error) => {
        showStatus(friendlyOpenError(error), { sticky: true });
      });
      return;
    }
    if (
      (event.ctrlKey || event.metaKey) &&
      !event.altKey &&
      event.key.toLowerCase() === "a"
    ) {
      event.preventDefault();
      sendBrowserInput(
        { type: "key_combo", keysyms: [65507, 97] },
        { history: "replace" },
      ).catch((error) => {
        showStatus(friendlyOpenError(error), { sticky: true });
      });
      return;
    }
    if (event.ctrlKey || event.metaKey || event.altKey) {
      return;
    }
    if (typeof event.key === "string" && [...event.key].length === 1) {
      event.preventDefault();
      sendBrowserInput(
        { type: "paste_text", text: event.key },
        { history: "replace" },
      ).catch((error) => {
        showStatus(friendlyOpenError(error), { sticky: true });
      });
      return;
    }
    const allowed =
      event.key === "Enter" ||
      event.key === "Backspace" ||
      event.key === "Delete" ||
      event.key === "Escape" ||
      event.key === "Tab" ||
      event.key === "ArrowUp" ||
      event.key === "ArrowDown" ||
      event.key === "ArrowLeft" ||
      event.key === "ArrowRight" ||
      event.key === "Home" ||
      event.key === "End" ||
      event.key === "PageUp" ||
      event.key === "PageDown";
    if (!allowed) {
      return;
    }
    event.preventDefault();
    sendBrowserInput(
      { type: "key", key: event.key },
      { history: event.key === "Enter" ? "push" : "replace" },
    ).catch((error) => {
      showStatus(friendlyOpenError(error), { sticky: true });
    });
  });

  renderPanel.addEventListener("paste", (event) => {
    const text = plainTextFromClipboardEvent(event);
    if (!getCurrentPage() || !text) {
      return;
    }
    unlockRemoteAudioFromGesture();
    event.preventDefault();
    pasteHostClipboardIntoRemote(text).catch((error) => {
      showStatus(friendlyOpenError(error), { sticky: true });
    });
  });

  keyboardCapture?.addEventListener("input", () => {
    keyboardCapture.value = "";
  });

  document.addEventListener(
    "keydown",
    (event) => {
      updateHostModifierState(event, true);
      handlePasteChord(event);
    },
    { capture: true },
  );
  document.addEventListener(
    "keyup",
    (event) => updateHostModifierState(event, false),
    { capture: true },
  );
  window.addEventListener("blur", () => {
    hostModifierState.control = false;
    hostModifierState.meta = false;
  });

  renderPanel.addEventListener("copy", (event) => {
    if (!getCurrentPage()) {
      return;
    }
    unlockRemoteAudioFromGesture();
    event.preventDefault();
    copyRemoteClipboardToHost().catch((error) => {
      showStatus(friendlyOpenError(error), { sticky: true });
    });
  });
}
