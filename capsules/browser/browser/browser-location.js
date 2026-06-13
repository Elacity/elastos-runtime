import { commitHistoryState } from "./browser-history.js?v=browser-20260520e";

const ADDRESS_EDIT_STALE_MS = 15000;

export function createBrowserLocationController({
  addressInput,
  updateNavState,
  titleSuffix = "ElastOS Browser",
}) {
  let currentUrl = addressInput?.value || "";
  let historyEntries = [];
  let historyIndex = -1;
  let addressDraftDirty = false;
  let addressDraftEditedAt = 0;

  function commitHistory(url, mode) {
    const next = commitHistoryState(
      { entries: historyEntries, index: historyIndex },
      url,
      mode,
    );
    historyEntries = next.entries;
    historyIndex = next.index;
    updateNavState();
  }

  function isAddressEditing() {
    if (document.activeElement === addressInput) {
      return true;
    }
    return (
      addressDraftDirty &&
      Date.now() - addressDraftEditedAt < ADDRESS_EDIT_STALE_MS
    );
  }

  function markAddressDraftEdited() {
    addressDraftDirty = true;
    addressDraftEditedAt = Date.now();
  }

  function clearAddressDraft() {
    addressDraftDirty = false;
    addressDraftEditedAt = 0;
  }

  function shouldUpdateAddressBar(force = false) {
    return force || !isAddressEditing();
  }

  function setCurrentUrl(url, { updateAddress = true, blur = false } = {}) {
    currentUrl = url;
    if (updateAddress) {
      addressInput.value = url;
    }
    if (blur) {
      addressInput.blur();
    }
  }

  function resetAddressToCurrent() {
    clearAddressDraft();
    addressInput.value = currentUrl;
    addressInput.blur();
  }

  function getCurrentUrl() {
    return currentUrl;
  }

  function syncBrowserLocation(
    url,
    title = "",
    mode = "push",
    { forceAddress = false } = {},
  ) {
    if (!url || typeof url !== "string" || !/^https?:\/\//i.test(url)) {
      return;
    }
    if (url === currentUrl) {
      if (shouldUpdateAddressBar(forceAddress)) {
        addressInput.value = url;
      }
      if (title) {
        document.title = `${title} - ${titleSuffix}`;
      }
      if (historyIndex < 0 || historyEntries[historyIndex] !== url) {
        commitHistory(url, mode);
      }
      updateNavState();
      return;
    }
    currentUrl = url;
    if (shouldUpdateAddressBar(forceAddress)) {
      addressInput.value = url;
    }
    if (title) {
      document.title = `${title} - ${titleSuffix}`;
    }
    commitHistory(url, mode);
  }

  return {
    clearAddressDraft,
    getCurrentUrl,
    isAddressEditing,
    markAddressDraftEdited,
    resetAddressToCurrent,
    setCurrentUrl,
    syncBrowserLocation,
  };
}
