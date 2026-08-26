const hashParams = new URLSearchParams(window.location.hash.replace(/^#/, ""));
const homeToken = hashParams.get("home_token") || "";
const homeOrigin = hashParams.get("home_origin") || "null";
const statusNode = document.getElementById("assistant-status");

function setStatus(message) {
  if (!statusNode) {
    return;
  }
  if (message) {
    statusNode.hidden = false;
    statusNode.textContent = message;
    return;
  }
  statusNode.hidden = true;
  statusNode.textContent = "";
}

async function loadOffers() {
  if (!homeToken) {
    setStatus("Model provider unavailable.");
    return;
  }
  try {
    const response = await fetch("/api/provider/model/offers_list", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        "x-elastos-home-token": homeToken,
      },
      body: "{}",
    });
    const payload = await response.json();
    if (!response.ok || payload?.status === "error") {
      setStatus("Model provider unavailable.");
      return;
    }
    const offers = Array.isArray(payload?.offers)
      ? payload.offers
      : Array.isArray(payload?.data?.offers)
        ? payload.data.offers
        : null;
    if (!offers) {
      setStatus("Model provider unavailable.");
      return;
    }
    if (offers.length === 0) {
      setStatus("No model offers available.");
      return;
    }
    setStatus("");
  } catch (_error) {
    setStatus("Model provider unavailable.");
  }
}

if (window.top) {
  window.top.postMessage(
    {
      type: "home:app-ready",
      homeToken,
    },
    homeOrigin,
  );
}

void loadOffers();
