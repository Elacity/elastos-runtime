(function () {
  var params = new URLSearchParams(location.search);
  var instance = params.get("browser_instance") || "";
  var pageId = params.get("page_id") || "";
  var generationHint = params.get("display_generation") || "";
  var token = new URLSearchParams(location.hash.replace(/^#/, "")).get("home_token") || "";
  if (!instance || !token) return;

  function validRequest(value) {
    return typeof value === "string" && /^[a-f0-9]{32}$/.test(value);
  }

  function validGeneration(value) {
    return typeof value === "string" && /^display:[a-f0-9]{32}$/.test(value);
  }

  function requestIdFromGeneration(generation) {
    return generation.indexOf("display:") === 0 ? generation.slice("display:".length) : "";
  }

  var retryIdentity = { page_id: "", generation: "", request_id: "" };

  function attachTimeoutMs() {
    return typeof VIEWER_DISPLAY_ATTACH_TIMEOUT_MS === "number" && VIEWER_DISPLAY_ATTACH_TIMEOUT_MS > 0
      ? VIEWER_DISPLAY_ATTACH_TIMEOUT_MS
      : 8000;
  }

  function reuseAttachment(pending) {
    if (
      !pending ||
      pending.schema !== "elastos.browser.display-attachment/v1" ||
      !validRequest(pending.request_id) ||
      !validGeneration(pending.previous_display_generation)
    ) {
      return null;
    }
    if (pending.state === "pending" || pending.state === "ready") return pending;
    if (pending.state === "failed" && pending.error_code === "display_attach_uncertain") return pending;
    return null;
  }

  function allocateRetryRequestId(targetPageId, generation) {
    if (
      retryIdentity.page_id === targetPageId &&
      retryIdentity.generation === generation &&
      validRequest(retryIdentity.request_id)
    ) {
      return retryIdentity.request_id;
    }
    retryIdentity.page_id = targetPageId;
    retryIdentity.generation = generation;
    retryIdentity.request_id = crypto.randomUUID().replace(/-/g, "");
    return retryIdentity.request_id;
  }

  function displayAttachRequest(targetPageId, generation, pending) {
    var reused = reuseAttachment(pending);
    if (reused) {
      return {
        type: "display_attach",
        request_id: reused.request_id,
        display_generation: reused.previous_display_generation,
      };
    }
    if (
      (pending && pending.state === "failed" && pending.error_code === "display_attach_failed") ||
      (validRequest(retryIdentity.request_id) &&
        retryIdentity.page_id === targetPageId &&
        retryIdentity.generation === generation)
    ) {
      return {
        type: "display_attach",
        request_id: allocateRetryRequestId(targetPageId, generation),
        display_generation: generation,
      };
    }
    return {
      type: "display_attach",
      request_id: generation.indexOf("display:") === 0
        ? generation.slice("display:".length)
        : crypto.randomUUID().replace(/-/g, ""),
      display_generation: generation,
    };
  }

  function postAttach(targetPageId, generation, pending) {
    var request = displayAttachRequest(targetPageId, generation, pending);
    var controller = typeof AbortController === "function" ? new AbortController() : null;
    var timer = controller ? setTimeout(function () { controller.abort(); }, attachTimeoutMs()) : 0;
    return fetch("/api/apps/browser/pages/" + encodeURIComponent(targetPageId) + "/webrtc", {
      method: "POST",
      headers: { "x-elastos-home-token": token, "content-type": "application/json" },
      body: JSON.stringify(request),
      signal: controller ? controller.signal : undefined,
    }).then(function (response) {
      if (!response.ok) throw new Error("Browser display attach failed");
      return response.json();
    }).then(function (result) {
      return { request: request, result: result, page_id: targetPageId };
    }).finally(function () {
      if (timer) clearTimeout(timer);
    });
  }

  function offerValid(offer) {
    return offer &&
      offer.schema === "elastos.browser.webrtc-offer/v1" &&
      offer.type === "offer" &&
      typeof offer.sdp === "string" &&
      offer.sdp.length > 0 &&
      offer.sdp.length <= 256 * 1024;
  }

  function resultFromSummary(summary, targetPageId, generation) {
    var recovery = summary && summary.sessions && summary.sessions.recoverable_page;
    var display = recovery && recovery.engine_page && recovery.engine_page.display_session;
    var pending = reuseAttachment(recovery && recovery.display_attachment);
    if (
      !recovery ||
      recovery.page_id !== targetPageId ||
      !pending ||
      pending.state !== "ready" ||
      pending.previous_display_generation !== generation ||
      !display ||
      !validGeneration(display.display_generation) ||
      display.display_generation === generation ||
      !offerValid(display.initial_offer) ||
      !offerValid(display.audio_offer)
    ) {
      return null;
    }
    return {
      request: {
        type: "display_attach",
        request_id: pending.request_id,
        display_generation: generation,
      },
      result: {
        schema: "elastos.browser.display-attach-result/v1",
        page_id: targetPageId,
        request_id: pending.request_id,
        previous_display_generation: generation,
        display_generation: display.display_generation,
        initial_offer: display.initial_offer,
        audio_offer: display.audio_offer,
      },
      page_id: targetPageId,
    };
  }

  function matchingAttachment(summary, targetPageId, generation) {
    var recovery = summary && summary.sessions && summary.sessions.recoverable_page;
    var pending = recovery && recovery.display_attachment;
    if (
      !recovery ||
      recovery.page_id !== targetPageId ||
      !pending ||
      pending.schema !== "elastos.browser.display-attachment/v1" ||
      ["pending", "ready", "failed"].indexOf(pending.state) < 0 ||
      !validRequest(pending.request_id) ||
      !validGeneration(pending.previous_display_generation) ||
      pending.previous_display_generation !== generation
    ) {
      return null;
    }
    return pending;
  }

  var headers = { "x-elastos-home-token": token };
  function fetchSummary() {
    return fetch(
      "/api/apps/browser/summary?browser_instance=" + encodeURIComponent(instance) + "&remote_services=0",
      { headers: headers },
    ).then(function (response) {
      if (!response.ok) throw new Error("Browser summary failed");
      return response.json();
    });
  }

  function delay(ms) {
    return new Promise(function (resolve) { setTimeout(resolve, ms); });
  }

  function announceReady(attached, fromSummary) {
    try {
      console.info(JSON.stringify({
        schema: "elastos.browser.media-diagnostic/v1",
        event: "restore_boot_ready",
        from_summary: fromSummary === true,
        request_id: attached && attached.request ? attached.request.request_id : "",
      }));
    } catch (error) {
      // Keep restore attach on the product path if diagnostics cannot print.
    }
    return attached;
  }

  function attachFromKnownUrl(targetPageId, generation) {
    return postAttach(targetPageId, generation, null).then(function (attached) {
      return announceReady(attached, false);
    }).catch(function () {
      return summaryPromise.then(function (summary) {
        var pending = matchingAttachment(summary, targetPageId, generation);
        if (!pending) {
          pending = summary && summary.sessions && summary.sessions.recoverable_page
            ? summary.sessions.recoverable_page.display_attachment
            : null;
        }
        return postAttach(targetPageId, generation, pending).then(function (attached) {
          return announceReady(attached, false);
        });
      });
    });
  }

  function waitForReadyAttach(targetPageId, generation) {
    var deadline = Date.now() + 2500;
    var missDeadline = Date.now() + 120;
    var seenPending = false;
    function step(summary) {
      var ready = resultFromSummary(summary, targetPageId, generation);
      if (ready) return announceReady(ready, true);
      var pending = matchingAttachment(summary, targetPageId, generation);
      if (pending) seenPending = true;
      if (pending && pending.state === "ready") {
        return postAttach(targetPageId, generation, pending).then(function (attached) {
          return announceReady(attached, false);
        });
      }
      if (pending && pending.state === "failed") {
        return postAttach(targetPageId, generation, pending);
      }
      var now = Date.now();
      if (seenPending && pending && pending.state === "pending" && now < deadline) {
        return delay(40).then(function () { return fetchSummary().then(step); });
      }
      if (!seenPending && now < missDeadline) {
        return delay(40).then(function () { return fetchSummary().then(step); });
      }
      return postAttach(targetPageId, generation, pending);
    }
    return summaryPromise.then(step);
  }

  function attachFromSummary(summary) {
    var recovery = summary && summary.sessions && summary.sessions.recoverable_page;
    var display = recovery && recovery.engine_page && recovery.engine_page.display_session;
    var generation = display && display.display_generation;
    var pending = recovery && recovery.display_attachment;
    if (
      !recovery ||
      recovery.state !== "active" ||
      typeof recovery.page_id !== "string" ||
      !validGeneration(generation) ||
      !summary.engine_adapter ||
      summary.engine_adapter.display_attach_supported !== true ||
      display.mode !== "webrtc_remote_display"
    ) {
      return null;
    }
    if (
      pending &&
      (pending.schema !== "elastos.browser.display-attachment/v1" ||
        ["pending", "ready", "failed"].indexOf(pending.state) < 0 ||
        !validRequest(pending.request_id) ||
        !validGeneration(pending.previous_display_generation))
    ) {
      return null;
    }
    if (pending && pending.state === "pending") {
      return waitForReadyAttach(recovery.page_id, pending.previous_display_generation);
    }
    return postAttach(recovery.page_id, generation, pending);
  }

  var summaryPromise = fetchSummary();

  var attachFromUrl = Boolean(pageId && validGeneration(generationHint));
  try {
    console.info(JSON.stringify({
      schema: "elastos.browser.media-diagnostic/v1",
      event: "restore_boot",
      attach_from_url: attachFromUrl,
      request_id: attachFromUrl ? requestIdFromGeneration(generationHint) : "",
    }));
  } catch (error) {
    // Keep boot attach on the product path if diagnostics cannot print.
  }
  var attachPromise = attachFromUrl
    ? attachFromKnownUrl(pageId, generationHint)
    : summaryPromise.then(attachFromSummary);

  window.__elastosBrowserRestoreBoot = {
    summaryPromise: summaryPromise,
    attachPromise: attachPromise,
    retryIdentity: retryIdentity,
  };
})();
