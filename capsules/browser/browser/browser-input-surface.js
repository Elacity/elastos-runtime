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
  let wheelDelta = { x: 0, y: 0 };
  let touchPanState = null;
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
