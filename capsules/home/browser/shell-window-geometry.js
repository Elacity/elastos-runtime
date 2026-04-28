import {
  desktop,
  windowSnapPreview,
  WINDOW_MIN_WIDTH,
  WINDOW_MIN_HEIGHT,
  WINDOW_SNAP_THRESHOLD,
  WINDOW_SIDE_INSET,
  WINDOW_TOP_INSET,
  WINDOW_BOTTOM_INSET,
  clamp,
} from "./shell-core.js?v=home-20260427b";

const WINDOW_MIN_VISIBLE_DRAG_WIDTH = 96;
const WINDOW_MIN_VISIBLE_DRAG_HEIGHT = 32;
const WINDOW_SNAP_ACTIVATION_DISTANCE = 12;

function safeClamp(value, min, max) {
  return max < min ? min : clamp(value, min, max);
}

function partialDragBoundsForWindow(width) {
  const workspaceRect = desktop.getBoundingClientRect();
  const workspaceWidth = Math.max(0, window.innerWidth - workspaceRect.left);
  const workspaceHeight = Math.max(0, window.innerHeight - workspaceRect.top);
  const visibleWidth = Math.min(WINDOW_MIN_VISIBLE_DRAG_WIDTH, Math.max(1, width));
  return {
    minX: Math.min(WINDOW_SIDE_INSET, visibleWidth - width),
    maxX: Math.max(WINDOW_SIDE_INSET, workspaceWidth - visibleWidth),
    minY: WINDOW_TOP_INSET - WINDOW_MIN_VISIBLE_DRAG_HEIGHT,
    maxY: Math.max(
      WINDOW_TOP_INSET,
      workspaceHeight - WINDOW_BOTTOM_INSET - WINDOW_MIN_VISIBLE_DRAG_HEIGHT,
    ),
  };
}

function fitWindowPosition({ x, y, width }, { allowPartial = false } = {}) {
  if (allowPartial) {
    const bounds = partialDragBoundsForWindow(width);
    return {
      x: safeClamp(x, bounds.minX, bounds.maxX),
      y: safeClamp(y, bounds.minY, bounds.maxY),
    };
  }

  const workspaceRect = desktop.getBoundingClientRect();
  const maxX = Math.max(
    WINDOW_SIDE_INSET,
    window.innerWidth - workspaceRect.left - width - WINDOW_SIDE_INSET,
  );
  return {
    x: safeClamp(x, WINDOW_SIDE_INSET, maxX),
    y,
  };
}

export function fitWindowBounds({ x, y, width, height }, { allowPartial = false } = {}) {
  const workspaceRect = desktop.getBoundingClientRect();
  const maxWidth = Math.max(
    WINDOW_MIN_WIDTH,
    window.innerWidth - workspaceRect.left - WINDOW_SIDE_INSET * 2,
  );
  const fittedWidth = Math.min(width, maxWidth);
  const maxWorkspaceHeight = Math.max(
    WINDOW_MIN_HEIGHT,
    window.innerHeight - workspaceRect.top - WINDOW_TOP_INSET - WINDOW_BOTTOM_INSET,
  );
  const provisionalHeight = Math.min(height, maxWorkspaceHeight);
  const maxY = Math.max(
    WINDOW_TOP_INSET,
    window.innerHeight - workspaceRect.top - provisionalHeight - WINDOW_BOTTOM_INSET,
  );
  const fittedPosition = fitWindowPosition(
    {
      x,
      y: allowPartial ? y : safeClamp(y, WINDOW_TOP_INSET, maxY),
      width: fittedWidth,
    },
    { allowPartial },
  );
  const fittedHeight = allowPartial
    ? Math.min(height, maxWorkspaceHeight)
    : Math.min(
        height,
        Math.max(
          WINDOW_MIN_HEIGHT,
          window.innerHeight - workspaceRect.top - fittedPosition.y - WINDOW_BOTTOM_INSET,
        ),
      );

  return {
    x: fittedPosition.x,
    y: fittedPosition.y,
    width: fittedWidth,
    height: fittedHeight,
  };
}

export function rememberWindowRestoreBounds(node) {
  if (node.dataset.maximized === "true" || node.dataset.snap) {
    return;
  }
  const bounds = normalWindowBounds(node);
  node.dataset.restoreLeft = `${bounds.x}px`;
  node.dataset.restoreTop = `${bounds.y}px`;
  node.dataset.restoreWidth = `${bounds.width}px`;
  node.dataset.restoreHeight = `${bounds.height}px`;
}

export function restoreWindowFromSpecialState(node) {
  node.dataset.maximized = "false";
  node.dataset.snap = "";
  if (
    node.dataset.restoreLeft &&
    node.dataset.restoreTop &&
    node.dataset.restoreWidth &&
    node.dataset.restoreHeight
  ) {
    node.style.left = node.dataset.restoreLeft;
    node.style.top = node.dataset.restoreTop;
    node.style.width = node.dataset.restoreWidth;
    node.style.height = node.dataset.restoreHeight;
  }
}

export function applyWindowPlacement(node, placement) {
  const restoreBounds = fitWindowBounds(
    {
      x: Number.isFinite(placement?.restoreX) ? placement.restoreX : Number.isFinite(placement?.x) ? placement.x : 48,
      y: Number.isFinite(placement?.restoreY) ? placement.restoreY : Number.isFinite(placement?.y) ? placement.y : 60,
      width: Number.isFinite(placement?.restoreWidth) ? placement.restoreWidth : Number.isFinite(placement?.width) ? placement.width : WINDOW_MIN_WIDTH,
      height: Number.isFinite(placement?.restoreHeight) ? placement.restoreHeight : Number.isFinite(placement?.height) ? placement.height : WINDOW_MIN_HEIGHT,
    },
    { allowPartial: true },
  );
  node.dataset.restoreLeft = `${restoreBounds.x}px`;
  node.dataset.restoreTop = `${restoreBounds.y}px`;
  node.dataset.restoreWidth = `${restoreBounds.width}px`;
  node.dataset.restoreHeight = `${restoreBounds.height}px`;

  if (placement?.maximized) {
    node.dataset.snap = "";
    node.dataset.maximized = "true";
    return;
  }

  node.dataset.maximized = "false";
  const snapState = typeof placement?.snap === "string" ? placement.snap : "";
  if (snapState === "left" || snapState === "right" || snapState === "nw" || snapState === "ne" || snapState === "sw" || snapState === "se") {
    node.dataset.snap = snapState;
    applyWindowBounds(node, snappedWindowBounds(snapState));
    return;
  }

  node.dataset.snap = "";
  applyWindowBounds(
    node,
    fitWindowBounds(
      {
        x: Number.isFinite(placement?.x) ? placement.x : restoreBounds.x,
        y: Number.isFinite(placement?.y) ? placement.y : restoreBounds.y,
        width: Number.isFinite(placement?.width) ? placement.width : restoreBounds.width,
        height: Number.isFinite(placement?.height) ? placement.height : restoreBounds.height,
      },
      { allowPartial: true },
    ),
  );
}

export function hideWindowSnapPreview() {
  windowSnapPreview.hidden = true;
}

export function attachWindowDrag(windowNode, handle, focusWindow, onWindowGeometryChange = null) {
  let dragging = null;

  handle.addEventListener("pointerdown", (event) => {
    if (event.target.closest("button")) {
      return;
    }
    hideWindowSnapPreview();
    focusWindow(windowNode.dataset.windowId);
    const workspaceRect = desktop.getBoundingClientRect();
    let offsetX;
    let offsetY;
    if (windowNode.dataset.maximized === "true" || windowNode.dataset.snap) {
      const restoredBounds = restoreWindowForDrag(windowNode, event.clientX, event.clientY);
      offsetX = event.clientX - workspaceRect.left - restoredBounds.x;
      offsetY = event.clientY - workspaceRect.top - restoredBounds.y;
    } else {
      const bounds = normalWindowBounds(windowNode);
      offsetX = event.clientX - workspaceRect.left - bounds.x;
      offsetY = event.clientY - workspaceRect.top - bounds.y;
    }
    dragging = {
      offsetX,
      offsetY,
      pointerId: event.pointerId,
      startClientX: event.clientX,
      startClientY: event.clientY,
      snapTarget: null,
    };
    handle.setPointerCapture(event.pointerId);
  });

  handle.addEventListener("pointermove", (event) => {
    if (!dragging || event.pointerId !== dragging.pointerId) {
      return;
    }

    const workspaceRect = desktop.getBoundingClientRect();
    const bounds = partialDragBoundsForWindow(windowNode.offsetWidth);
    const left = safeClamp(
      event.clientX - dragging.offsetX - workspaceRect.left,
      bounds.minX,
      bounds.maxX,
    );
    const top = safeClamp(
      event.clientY - dragging.offsetY - workspaceRect.top,
      bounds.minY,
      bounds.maxY,
    );
    windowNode.style.left = `${left}px`;
    windowNode.style.top = `${top}px`;
    const movedDistance = Math.hypot(
      event.clientX - dragging.startClientX,
      event.clientY - dragging.startClientY,
    );
    dragging.snapTarget = movedDistance >= WINDOW_SNAP_ACTIVATION_DISTANCE
      ? snapTargetForPointer(event.clientX, event.clientY)
      : null;
    showWindowSnapPreview(dragging.snapTarget);
  });

  const stopDrag = (event) => {
    if (!dragging || event.pointerId !== dragging.pointerId) {
      return;
    }
    const snapTarget = dragging.snapTarget;
    dragging = null;
    try {
      handle.releasePointerCapture(event.pointerId);
    } catch (_error) {
      // Pointer capture may already be released.
    }
    if (event.type === "pointerup" && snapTarget) {
      applyWindowSnap(windowNode, snapTarget);
      focusWindow(windowNode.dataset.windowId);
      if (typeof onWindowGeometryChange === "function") {
        onWindowGeometryChange();
      }
      return;
    }
    hideWindowSnapPreview();
    if (event.type === "pointerup" && typeof onWindowGeometryChange === "function") {
      onWindowGeometryChange();
    }
  };

  handle.addEventListener("pointerup", stopDrag);
  handle.addEventListener("pointercancel", stopDrag);
}

export function attachWindowResize(windowNode, focusWindow, onWindowGeometryChange = null) {
  for (const handle of windowNode.querySelectorAll(".window-resize-handle")) {
    let resizing = null;
    const directions = handle.dataset.resize || "";

    handle.addEventListener("pointerdown", (event) => {
      if (windowNode.dataset.maximized === "true") {
        return;
      }
      event.stopPropagation();
      hideWindowSnapPreview();
      if (windowNode.dataset.snap) {
        windowNode.dataset.snap = "";
      }
      focusWindow(windowNode.dataset.windowId);
      const bounds = normalWindowBounds(windowNode);
      resizing = {
        directions,
        pointerId: event.pointerId,
        startClientX: event.clientX,
        startClientY: event.clientY,
        left: bounds.x,
        top: bounds.y,
        right: bounds.x + bounds.width,
        bottom: bounds.y + bounds.height,
      };
      handle.setPointerCapture(event.pointerId);
    });

    handle.addEventListener("pointermove", (event) => {
      if (!resizing || event.pointerId !== resizing.pointerId) {
        return;
      }

      const workspaceRect = desktop.getBoundingClientRect();
      const maxRight = window.innerWidth - workspaceRect.left - WINDOW_SIDE_INSET;
      const maxBottom = window.innerHeight - workspaceRect.top - WINDOW_BOTTOM_INSET;
      let left = resizing.left;
      let top = resizing.top;
      let right = resizing.right;
      let bottom = resizing.bottom;

      if (resizing.directions.includes("e")) {
        right = clamp(
          resizing.right + (event.clientX - resizing.startClientX),
          left + WINDOW_MIN_WIDTH,
          maxRight,
        );
      }
      if (resizing.directions.includes("s")) {
        bottom = clamp(
          resizing.bottom + (event.clientY - resizing.startClientY),
          top + WINDOW_MIN_HEIGHT,
          maxBottom,
        );
      }
      if (resizing.directions.includes("w")) {
        left = clamp(
          resizing.left + (event.clientX - resizing.startClientX),
          WINDOW_SIDE_INSET,
          right - WINDOW_MIN_WIDTH,
        );
      }
      if (resizing.directions.includes("n")) {
        top = clamp(
          resizing.top + (event.clientY - resizing.startClientY),
          WINDOW_TOP_INSET,
          bottom - WINDOW_MIN_HEIGHT,
        );
      }

      applyWindowBounds(windowNode, {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
      });
    });

    const stopResize = (event) => {
      if (!resizing || event.pointerId !== resizing.pointerId) {
        return;
      }
      resizing = null;
      try {
        handle.releasePointerCapture(event.pointerId);
      } catch (_error) {
        // Pointer capture may already be released.
      }
      if (event.type === "pointerup" && typeof onWindowGeometryChange === "function") {
        onWindowGeometryChange();
      }
    };

    handle.addEventListener("pointerup", stopResize);
    handle.addEventListener("pointercancel", stopResize);
  }
}

function normalWindowBounds(node) {
  return {
    x: Number.parseFloat(node.style.left) || node.offsetLeft || 48,
    y: Number.parseFloat(node.style.top) || node.offsetTop || 60,
    width: Number.parseFloat(node.style.width) || node.offsetWidth || WINDOW_MIN_WIDTH,
    height: Number.parseFloat(node.style.height) || node.offsetHeight || WINDOW_MIN_HEIGHT,
  };
}

function applyWindowBounds(node, bounds) {
  node.style.left = `${bounds.x}px`;
  node.style.top = `${bounds.y}px`;
  node.style.width = `${bounds.width}px`;
  node.style.height = `${bounds.height}px`;
}

function savedWindowRestoreBounds(node) {
  return fitWindowBounds({
    x: Number.parseFloat(node.dataset.restoreLeft) || Number.parseFloat(node.style.left) || node.offsetLeft || 48,
    y: Number.parseFloat(node.dataset.restoreTop) || Number.parseFloat(node.style.top) || node.offsetTop || 60,
    width: Number.parseFloat(node.dataset.restoreWidth) || Number.parseFloat(node.style.width) || node.offsetWidth || WINDOW_MIN_WIDTH,
    height: Number.parseFloat(node.dataset.restoreHeight) || Number.parseFloat(node.style.height) || node.offsetHeight || WINDOW_MIN_HEIGHT,
  });
}

function restoreWindowForDrag(node, clientX, clientY) {
  const workspaceRect = desktop.getBoundingClientRect();
  const currentRect = node.getBoundingClientRect();
  const restoreBounds = savedWindowRestoreBounds(node);
  const pointerRatioX = currentRect.width > 0
    ? clamp((clientX - currentRect.left) / currentRect.width, 0.16, 0.84)
    : 0.5;
  const pointerOffsetY = clamp(clientY - currentRect.top, 14, 30);
  const nextBounds = fitWindowBounds({
    x: clientX - workspaceRect.left - restoreBounds.width * pointerRatioX,
    y: clientY - workspaceRect.top - pointerOffsetY,
    width: restoreBounds.width,
    height: restoreBounds.height,
  });
  restoreWindowFromSpecialState(node);
  applyWindowBounds(node, nextBounds);
  return nextBounds;
}

function snappedWindowBounds(state) {
  const workspaceRect = desktop.getBoundingClientRect();
  const availableWidth = Math.max(
    WINDOW_MIN_WIDTH,
    window.innerWidth - workspaceRect.left - WINDOW_SIDE_INSET * 2,
  );
  const availableHeight = Math.max(
    WINDOW_MIN_HEIGHT,
    window.innerHeight - workspaceRect.top - WINDOW_TOP_INSET - WINDOW_BOTTOM_INSET,
  );
  if (state === "maximize") {
    return {
      x: WINDOW_SIDE_INSET,
      y: WINDOW_TOP_INSET,
      width: availableWidth,
      height: availableHeight,
    };
  }
  const gutter = 8;
  const width = Math.max(WINDOW_MIN_WIDTH, Math.floor((availableWidth - gutter) / 2));
  const height = Math.max(WINDOW_MIN_HEIGHT, Math.floor((availableHeight - gutter) / 2));
  if (state === "nw" || state === "ne" || state === "sw" || state === "se") {
    return {
      x: state.endsWith("w") ? WINDOW_SIDE_INSET : WINDOW_SIDE_INSET + availableWidth - width,
      y: state.startsWith("n") ? WINDOW_TOP_INSET : WINDOW_TOP_INSET + availableHeight - height,
      width,
      height,
    };
  }
  return {
    x: state === "left" ? WINDOW_SIDE_INSET : WINDOW_SIDE_INSET + availableWidth - width,
    y: WINDOW_TOP_INSET,
    width,
    height: availableHeight,
  };
}

function snapTargetForPointer(clientX, clientY) {
  const workspaceRect = desktop.getBoundingClientRect();
  const snapBottom = window.innerHeight - WINDOW_BOTTOM_INSET;
  const insideHorizontal = clientX >= workspaceRect.left && clientX <= window.innerWidth;
  const insideVertical = clientY >= workspaceRect.top && clientY <= snapBottom;
  const nearTop =
    insideHorizontal &&
    clientY >= workspaceRect.top &&
    clientY <= workspaceRect.top + WINDOW_SNAP_THRESHOLD;
  const nearLeft =
    insideVertical &&
    clientX >= workspaceRect.left &&
    clientX <= workspaceRect.left + WINDOW_SNAP_THRESHOLD;
  const nearRight =
    insideVertical &&
    clientX <= window.innerWidth &&
    clientX >= window.innerWidth - WINDOW_SNAP_THRESHOLD;
  const nearBottom =
    insideHorizontal &&
    clientY <= snapBottom &&
    clientY >= snapBottom - WINDOW_SNAP_THRESHOLD;
  if (nearTop && nearLeft) {
    return { state: "nw", bounds: snappedWindowBounds("nw") };
  }
  if (nearTop && nearRight) {
    return { state: "ne", bounds: snappedWindowBounds("ne") };
  }
  if (nearBottom && nearLeft) {
    return { state: "sw", bounds: snappedWindowBounds("sw") };
  }
  if (nearBottom && nearRight) {
    return { state: "se", bounds: snappedWindowBounds("se") };
  }
  if (nearTop) {
    return { state: "maximize", bounds: snappedWindowBounds("maximize") };
  }
  if (nearLeft) {
    return { state: "left", bounds: snappedWindowBounds("left") };
  }
  if (nearRight) {
    return { state: "right", bounds: snappedWindowBounds("right") };
  }
  return null;
}

function showWindowSnapPreview(target) {
  if (!target) {
    hideWindowSnapPreview();
    return;
  }
  windowSnapPreview.hidden = false;
  windowSnapPreview.style.left = `${target.bounds.x}px`;
  windowSnapPreview.style.top = `${target.bounds.y}px`;
  windowSnapPreview.style.width = `${target.bounds.width}px`;
  windowSnapPreview.style.height = `${target.bounds.height}px`;
}

function applyWindowSnap(node, target) {
  if (!target) {
    return;
  }
  rememberWindowRestoreBounds(node);
  hideWindowSnapPreview();
  if (target.state === "maximize") {
    node.dataset.snap = "";
    node.dataset.maximized = "true";
    return;
  }
  node.dataset.maximized = "false";
  node.dataset.snap = target.state;
  applyWindowBounds(node, target.bounds);
}
