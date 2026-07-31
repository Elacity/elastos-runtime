function walletApprovalError(message, code) {
  const error = new Error(message);
  error.code = code;
  return error;
}

export function walletApprovalDeadlineMs(expiresAt, nowMs) {
  if (!Number.isSafeInteger(expiresAt)) {
    throw walletApprovalError(
      "Runtime wallet approval response did not include a valid expiry.",
      4100,
    );
  }
  const deadlineMs = expiresAt * 1000;
  if (!Number.isSafeInteger(deadlineMs)) {
    throw walletApprovalError(
      "Runtime wallet approval response did not include a valid expiry.",
      4100,
    );
  }
  if (deadlineMs <= nowMs) {
    throw walletApprovalError(
      "Wallet request expired before approval.",
      4001,
    );
  }
  if (deadlineMs - nowMs > 30 * 60 * 1000) {
    throw walletApprovalError(
      "Runtime wallet approval expiry exceeds the maximum wait.",
      4100,
    );
  }
  return deadlineMs;
}

function isTerminalWalletApproval(status) {
  return ["completed", "rejected", "expired"].includes(status?.status);
}

function walletApprovalStatusTimeoutError() {
  return walletApprovalError(
    "Runtime wallet approval status observation timed out.",
    4100,
  );
}

function withWalletApprovalStatusTimeout(promise, timeoutMs) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const settle = (complete, value) => {
      if (settled) {
        return;
      }
      settled = true;
      clearTimeout(timer);
      complete(value);
    };
    const timer = setTimeout(
      () => settle(reject, walletApprovalStatusTimeoutError()),
      timeoutMs,
    );
    Promise.resolve(promise).then(
      (value) => settle(resolve, value),
      (error) => settle(reject, error),
    );
  });
}

async function observeWalletApprovalStatus(
  requestId,
  timeoutMs,
  { getStatus, withStatusTimeout },
) {
  try {
    return await withStatusTimeout(
      Promise.resolve().then(() => getStatus(requestId, { timeoutMs })),
      timeoutMs,
    );
  } catch {
    return null;
  }
}

export async function waitForWalletApprovalStatus(
  requestId,
  expiresAt,
  {
    getStatus,
    now = Date.now,
    wait = (delayMs) => new Promise((resolve) => setTimeout(resolve, delayMs)),
    pollIntervalMs = 1000,
    statusIoTimeoutMs = 3000,
    withStatusTimeout = withWalletApprovalStatusTimeout,
  },
) {
  const deadlineMs = walletApprovalDeadlineMs(expiresAt, now());
  while (now() < deadlineMs) {
    const status = await observeWalletApprovalStatus(
      requestId,
      Math.max(1, Math.min(statusIoTimeoutMs, deadlineMs - now())),
      { getStatus, withStatusTimeout },
    );
    if (isTerminalWalletApproval(status)) {
      return status;
    }
    const remainingMs = deadlineMs - now();
    if (remainingMs <= 0) {
      break;
    }
    await wait(Math.min(pollIntervalMs, remainingMs));
  }
  const finalStatus = await observeWalletApprovalStatus(
    requestId,
    statusIoTimeoutMs,
    { getStatus, withStatusTimeout },
  );
  if (isTerminalWalletApproval(finalStatus)) {
    return finalStatus;
  }
  throw walletApprovalError(
    "Wallet request timed out before approval.",
    4001,
  );
}

export async function waitForWalletApprovalSignature(
  requestId,
  expiresAt,
  options,
) {
  const status = await waitForWalletApprovalStatus(
    requestId,
    expiresAt,
    options,
  );
  if (status.status === "completed") {
    if (typeof status.signature === "string" && status.signature.trim() !== "") {
      return status.signature;
    }
    throw walletApprovalError(
      "Runtime wallet approval completed without a signature.",
      4100,
    );
  }
  if (status.status === "rejected") {
    throw walletApprovalError(
      "Wallet request was rejected in ElastOS Wallet/Inbox.",
      4001,
    );
  }
  throw walletApprovalError(
    "Wallet request expired before approval.",
    4001,
  );
}

export async function waitForWalletApprovalTransaction(
  requestId,
  expiresAt,
  { broadcastTransaction, ...options },
) {
  const status = await waitForWalletApprovalStatus(
    requestId,
    expiresAt,
    options,
  );
  if (status.status === "rejected") {
    throw walletApprovalError(
      "Wallet request was rejected in ElastOS Wallet/Inbox.",
      4001,
    );
  }
  if (status.status === "expired") {
    throw walletApprovalError(
      "Wallet request expired before approval.",
      4001,
    );
  }
  if (
    typeof status.transaction_hash === "string" &&
    status.transaction_hash.trim() !== ""
  ) {
    return status.transaction_hash;
  }
  if (
    typeof status.signed_transaction !== "string" ||
    status.signed_transaction.trim() === ""
  ) {
    throw walletApprovalError(
      "Runtime wallet approval completed without a signed transaction.",
      4100,
    );
  }
  const receipt = await broadcastTransaction(requestId);
  if (
    typeof receipt?.transaction_hash !== "string" ||
    receipt.transaction_hash.trim() === ""
  ) {
    throw walletApprovalError(
      "Runtime transaction broadcast did not return a transaction hash.",
      4100,
    );
  }
  return receipt.transaction_hash;
}

export function cacheWalletApprovalPromise(cache, cacheKey, create, onReuse) {
  const existing = cache.get(cacheKey);
  if (existing) {
    onReuse?.(existing);
    return existing.promise;
  }
  const entry = { promise: null, requestSuffix: "" };
  const pending = Promise.resolve().then(() => create(entry));
  entry.promise = pending.finally(() => {
    if (cache.get(cacheKey) === entry) {
      cache.delete(cacheKey);
    }
  });
  cache.set(cacheKey, entry);
  return entry.promise;
}
