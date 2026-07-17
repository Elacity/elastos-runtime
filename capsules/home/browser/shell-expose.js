/* Exposé-lite — scale every visible window into a glanceable grid using
 * transform/opacity only (no layout thrash). F3 toggles; click a card to
 * focus that window; Esc dismisses. Pure shell presentation — no gateway.
 */

import { shellState } from "./shell-core.js?v=home-20260701c";
import {
  browserWindowEntries,
  focusWindow,
  sortWindowEntriesByZOrder,
} from "./shell-windows.js?v=home-20260701c";

const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
let active = false;
const originals = new Map(); // windowId -> { transform, transition, zIndex, boxShadow }

function visibleEntries() {
  return sortWindowEntriesByZOrder(browserWindowEntries()).filter(
    (entry) => !entry.node.classList.contains("hidden"),
  );
}

function gridSpec(count) {
  const cols = Math.ceil(Math.sqrt(count));
  const rows = Math.ceil(count / cols);
  return { cols, rows };
}

export function isExposeOpen() {
  return active;
}

export function closeExpose() {
  if (!active) {
    return;
  }
  active = false;
  document.body.classList.remove("expose-active");
  for (const [windowId, style] of originals) {
    const entry = shellState.windows.get(windowId);
    if (entry?.node) {
      entry.node.style.transform = style.transform;
      entry.node.style.transition = style.transition;
      entry.node.style.zIndex = style.zIndex;
      entry.node.style.boxShadow = style.boxShadow;
      entry.node.classList.remove("expose-card");
      entry.node.removeAttribute("data-expose");
    }
  }
  originals.clear();
}

export function openExpose() {
  const entries = visibleEntries();
  if (entries.length < 2) {
    return false;
  }
  closeExpose();
  active = true;
  document.body.classList.add("expose-active");
  const stage = document.querySelector("#desktop") || document.body;
  const stageRect = stage.getBoundingClientRect();
  const pad = 24;
  const { cols, rows } = gridSpec(entries.length);
  const cellW = (stageRect.width - pad * (cols + 1)) / cols;
  const cellH = (stageRect.height - pad * (rows + 1)) / rows;

  entries.forEach((entry, index) => {
    const node = entry.node;
    const rect = node.getBoundingClientRect();
    originals.set(entry.id, {
      transform: node.style.transform,
      transition: node.style.transition,
      zIndex: node.style.zIndex,
      boxShadow: node.style.boxShadow,
    });
    const col = index % cols;
    const row = Math.floor(index / cols);
    const targetX = stageRect.left + pad + col * (cellW + pad) + cellW / 2;
    const targetY = stageRect.top + pad + row * (cellH + pad) + cellH / 2;
    const scale = Math.min(cellW / Math.max(rect.width, 1), cellH / Math.max(rect.height, 1), 1) * 0.92;
    const centerX = rect.left + rect.width / 2;
    const centerY = rect.top + rect.height / 2;
    const dx = targetX - centerX;
    const dy = targetY - centerY;
    node.classList.add("expose-card");
    node.dataset.expose = entry.id;
    node.style.zIndex = String(200000 + index);
    node.style.boxShadow = "0 18px 50px rgba(0, 0, 0, 0.45)";
    if (!reducedMotion.matches) {
      node.style.transition = "transform 220ms cubic-bezier(0.2, 0, 0, 1)";
    }
    node.style.transform = `translate(${dx}px, ${dy}px) scale(${scale})`;
  });
  return true;
}

export function toggleExpose() {
  if (active) {
    closeExpose();
    return true;
  }
  return openExpose();
}

export function bindExpose() {
  document.addEventListener(
    "click",
    (event) => {
      if (!active) {
        return;
      }
      const card = event.target.closest(".window[data-expose]");
      if (card?.dataset.expose) {
        event.preventDefault();
        event.stopPropagation();
        const id = card.dataset.expose;
        closeExpose();
        focusWindow(id, { moveFocus: true });
        return;
      }
      if (!event.target.closest(".window")) {
        closeExpose();
      }
    },
    true,
  );
}
