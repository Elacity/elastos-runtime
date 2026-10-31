    const homeLaunchToken = new URLSearchParams(window.location.hash.replace(/^#/, "")).get("home_token") || "";
    const homeOrigin = new URLSearchParams(window.location.search).get("home_origin") || "";
    const accessMode = homeLaunchToken
      ? "shell"
      : "gateway";
    document.body.setAttribute("data-room-access-mode", accessMode);
