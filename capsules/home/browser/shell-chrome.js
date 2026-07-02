import { clockNode } from "./shell-core.js?v=home-20260701c";

export function syncIdentity(_summary) {}

export function clearIdentitySurface() {}

export function updateClock() {
  clockNode.textContent = new Intl.DateTimeFormat([], {
    hour: "numeric",
    minute: "2-digit",
    weekday: "short",
    month: "short",
    day: "2-digit",
  }).format(new Date());
}
