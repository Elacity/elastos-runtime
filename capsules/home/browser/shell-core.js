export const activeShellRoot = document.querySelector("#active-shell-root");
export const activeShellFrame = document.querySelector("#active-shell-frame");
export const homeShellBootMask = document.querySelector("#home-shell-boot-mask");
export const shellHostRecovery = document.querySelector("#shell-host-recovery");
export const shellHostRecoveryTitle = document.querySelector("#shell-host-recovery-title");
export const shellHostRecoveryCopy = document.querySelector("#shell-host-recovery-copy");
export const shellHostRecoveryDetail = document.querySelector("#shell-host-recovery-detail");
export const shellHostRecoveryHomeButton = document.querySelector("#shell-host-recovery-home");
export const shellHostRecoveryReloadButton = document.querySelector("#shell-host-recovery-reload");
export const shellHostRecoverySignOutButton = document.querySelector("#shell-host-recovery-sign-out");

export const HOME_SHELL_HOST_ID = "home-shell-host";
export const HOME_GUI_SHELL_ID = "home-gui";
export const SYSTEM_APP_ID = "system";
export const PEOPLE_TARGET_ID = "people";

export function homeActiveShellName(value) {
  return typeof value === "string" ? value.trim() : "";
}

export const shellState = {
  summaryRefreshDebounceTimer: null,
  summaryRefreshInFlight: false,
  summaryVisibilityRefreshBound: false,
  homeEventsCursor: "",
  homeEventsTimer: null,
  homeEventsInFlight: false,
  homeEventsSource: null,
  homeEventsStreamFailed: false,
  sessionRefreshTimer: null,
  presenceHeartbeatTimer: null,
  presenceHeartbeatInFlight: null,
  currentSummary: null,
  requestSummaryRefresh: null,
  activeShellRootTarget: "",
  activeShellRootRoute: "",
  activeShellRootHomeToken: "",
  activeShellRootLaunchSeq: 0,
  pendingAppliedShellTarget: "",
  homeGuiMounted: false,
};

let homeAuthorityToken = "";

export function setHomeAuthorityToken(value) {
  homeAuthorityToken = typeof value === "string" ? value.trim() : "";
}

export function clearHomeAuthorityToken() {
  homeAuthorityToken = "";
}

export function hasHomeAuthorityToken() {
  return Boolean(homeAuthorityToken);
}

export async function fetchJson(url, init) {
  const requiresHomeAuthority = url === "/api/apps/home/launch"
    || url === "/api/apps/home/active-shell"
    || url === "/api/apps/home/collaboration/presence";
  const authorityHeaders = requiresHomeAuthority && homeAuthorityToken
    ? { "x-elastos-home-token": homeAuthorityToken }
    : {};
  const response = await fetch(url, {
    ...init,
    headers: {
      "content-type": "application/json",
      ...authorityHeaders,
      ...(init && init.headers ? init.headers : {}),
    },
  });
  if (!response.ok) {
    const detail = await response.text().catch(() => "");
    const error = new Error(
      `request failed: ${response.status} ${response.statusText}${detail ? ` ${detail}` : ""}`,
    );
    error.status = response.status;
    throw error;
  }
  return response.json();
}

export function allVisibleTargets(summary) {
  if (!summary || !Array.isArray(summary.targets)) {
    return [];
  }
  return summary.targets.filter((target) => target?.role !== "shell");
}

export function targetById(summary, targetId) {
  return allVisibleTargets(summary).find((target) => target.target === targetId) || null;
}
