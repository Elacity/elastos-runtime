const WHEEL_FLUSH_MS = 80;

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
  captureViewerOwnerGuard,
  copyRemoteClipboardToHost,
  friendlyOpenError,
  getCurrentPage,
  getCurrentView,
  keyboardCapture,
  pasteHostClipboardIntoRemote,
  readHostClipboardText,
  remoteVideo,
  renderPanel,
  sendBrowserInput,
  showStatus,
  unlockRemoteAudioFromGesture,
}) {
  let wheelTimer = 0;
  let wheelBatch = null;
  let touchPanState = null;
  let suppressSyntheticClickUntil = 0;
  const hostModifierState = {
    control: false,
    meta: false,
  };

  function retireTouchPan() {
    const retired = touchPanState;
    // Drop ownership before releasing capture: lostpointercapture may run now.
    touchPanState = null;
    if (!retired) return null;
    suppressSyntheticClickUntil = Date.now() + 700;
    if (renderPanel.hasPointerCapture?.(retired.pointerId)) {
      renderPanel.releasePointerCapture(retired.pointerId);
    }
    return retired;
  }

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

  function browserPointFromEvent(event) {
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
    if (x < 0 || y < 0 || x > width || y > height) {
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

  function sendClickFromEvent(event, ownerActive = captureViewerOwnerGuard()) {
    if (!ownerActive()) {
      return;
    }
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
      if (ownerActive()) {
        showStatus(friendlyOpenError(error), { sticky: true });
      }
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
    if (wheelBatch && !wheelBatch.ownerActive()) {
      window.clearTimeout(wheelTimer);
      wheelBatch = null;
    }
    if (!wheelBatch) {
      const ownerActive = captureViewerOwnerGuard();
      if (!ownerActive()) {
        return;
      }
      wheelBatch = { ownerActive, x: 0, y: 0, point };
    }
    const batch = wheelBatch;
    batch.x += deltaX;
    batch.y += deltaY;
    batch.point = point;
    window.clearTimeout(wheelTimer);
    wheelTimer = window.setTimeout(() => {
      if (wheelBatch !== batch) {
        return;
      }
      wheelTimer = 0;
      wheelBatch = null;
      if (!batch.ownerActive()) {
        return;
      }
      sendBrowserInput(
        {
          type: "wheel",
          x: batch.point?.x,
          y: batch.point?.y,
          delta_x: batch.x,
          delta_y: batch.y,
        },
        { history: "replace" },
      ).catch((error) => {
        if (batch.ownerActive()) {
          showStatus(friendlyOpenError(error), { sticky: true });
        }
      });
    }, WHEEL_FLUSH_MS);
  }

  function isSyntheticTouchClick(event) {
    // Touch and pen taps dispatch once from pointerup under the captured
    // owner. Their later click event cannot capture a replacement owner.
    if (event.pointerType === "touch" || event.pointerType === "pen" ||
        event.sourceCapabilities?.firesTouchEvents === true) return true;
    // Older hosts report compatibility clicks without pointer provenance.
    // A known mouse remains independent from that short touch fallback.
    return event.pointerType !== "mouse" && Date.now() < suppressSyntheticClickUntil;
  }

  remoteVideo.addEventListener("click", (event) => {
    if (isSyntheticTouchClick(event)) {
      event.preventDefault();
      return;
    }
    event.stopPropagation();
    sendClickFromEvent(event);
  });

  renderPanel.addEventListener("click", (event) => {
    if (isSyntheticTouchClick(event)) {
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
      // An owner that has retired cannot reserve the next page's gesture.
      if (touchPanState && !touchPanState.ownerActive()) retireTouchPan();
      // The current owner keeps its first pointer until that gesture ends.
      if (touchPanState) {
        return;
      }
      if (!getCurrentPage()) {
        return;
      }
      const ownerActive = captureViewerOwnerGuard();
      if (!ownerActive()) {
        return;
      }
      event.preventDefault();
      focusKeyboardCapture();
      touchPanState = {
        pointerId: event.pointerId,
        ownerActive,
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
        event.pointerId !== touchPanState.pointerId
      ) {
        return;
      }
      event.preventDefault();
      if (!touchPanState.ownerActive() || !getCurrentPage()) {
        retireTouchPan();
        return;
      }
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
      // A retired finger can remain down longer than the fallback window.
      // Renew it at release without taking ownership from a fresh pointer.
      if (event.pointerType === "touch" || event.pointerType === "pen") {
        suppressSyntheticClickUntil = Date.now() + 700;
      }
      if (!touchPanState || event.pointerId !== touchPanState.pointerId) {
        return;
      }
      event.preventDefault();
      const { moved, ownerActive } = retireTouchPan();
      if (!moved) {
        sendClickFromEvent(event, ownerActive);
      }
    },
    { passive: false },
  );

  for (const type of ["pointercancel", "lostpointercapture"]) {
    renderPanel.addEventListener(type, (event) => {
      if (touchPanState?.pointerId === event.pointerId) retireTouchPan();
    });
  }

  renderPanel.addEventListener(
    "wheel",
    (event) => {
      if (!getCurrentPage()) {
        return;
      }
      unlockRemoteAudioFromGesture();
      const point = browserPointFromEvent(event);
      if (!point) {
        return;
      }
      event.preventDefault();
      focusKeyboardCapture();
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
