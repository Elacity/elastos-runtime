export function isPageStatusTimeout(error) {
  return error?.name === "AbortError" || error?.name === "TimeoutError";
}

export function pageStatusHttpError(status) {
  const error = new Error(`Recovery page status failed: ${status}`);
  error.name = "PageStatusHttpError";
  error.status = status;
  return error;
}

export function pageStatusJsonError(cause) {
  const error = new Error("Recovery page status JSON failed");
  error.name = "PageStatusJsonError";
  error.cause = cause;
  return error;
}

// Timeout reuse keeps the last matching Engine status. HTTP and JSON errors
// stay visible so a later 403/404/500 cannot pass as a fresh observation.
export async function readBrowserPageStatus({
  fetchImpl,
  url,
  headers,
  signal,
  budgetMs,
  lastPageStatus,
}) {
  const statusController = new AbortController();
  const onAbort = () => statusController.abort();
  signal?.addEventListener("abort", onAbort, { once: true });
  const timer = budgetMs != null ? setTimeout(() => statusController.abort(), budgetMs) : null;
  try {
    const response = await fetchImpl(url, { headers, signal: statusController.signal });
    if (!response.ok) throw pageStatusHttpError(response.status);
    let pageStatus;
    try {
      pageStatus = await response.json();
    } catch (error) {
      throw pageStatusJsonError(error);
    }
    return { page_status: pageStatus, page_status_fresh: true };
  } catch (error) {
    if (isPageStatusTimeout(error) && lastPageStatus) {
      return { page_status: lastPageStatus, page_status_fresh: false };
    }
    throw error;
  } finally {
    if (timer != null) clearTimeout(timer);
    signal?.removeEventListener("abort", onAbort);
  }
}
