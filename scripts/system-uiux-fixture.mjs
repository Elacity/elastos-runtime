import { createServer } from "node:http";
import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";
import { extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("../", import.meta.url)));
const systemRoot = join(repoRoot, "capsules/system/browser");
const homeRoot = join(repoRoot, "capsules/home/browser");
const homeGuiRoot = join(repoRoot, "capsules/home-gui/browser");

export const brave =
  process.env.BRAVE_BIN ||
  "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";

const require = createRequire(
  new URL("../elastos/tools/browser-playwright-engine/package.json", import.meta.url),
);

export const { chromium } = require("playwright");

export const CORS_HEADERS = Object.freeze({
  "access-control-allow-origin": "null",
  "access-control-allow-headers": "content-type,x-elastos-home-token",
  "access-control-allow-methods": "GET,POST,OPTIONS",
});

export function assert(condition, message, details = undefined) {
  if (!condition) {
    throw new Error(
      `${message}${details ? `\n${JSON.stringify(details, null, 2)}` : ""}`,
    );
  }
}

export function jsonResponse(response, value, status = 200) {
  const body = Buffer.from(JSON.stringify(value));
  response.writeHead(status, {
    ...CORS_HEADERS,
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": "application/json",
  });
  response.end(body);
}

export function textResponse(response, value, status = 200) {
  const body = Buffer.from(String(value));
  response.writeHead(status, {
    ...CORS_HEADERS,
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": "text/plain; charset=utf-8",
  });
  response.end(body);
}

export function makeAppearanceRecord(overrides = {}) {
  return {
    schema: "elastos.home.appearance/v1",
    revision: 5,
    theme: "dark",
    accent: "orange",
    accent_custom: "#4f7fff",
    dock_auto_hide: true,
    sounds: false,
    focus_mode: false,
    background_image_url: null,
    background_overlay_enabled: true,
    background_overlay_opacity: 0.55,
    ...overrides,
  };
}

export function makeSystemSummary(
  appearance,
  {
    proofBindingId = "proof:passkey:system-smoke",
    deviceDid = "did:key:z6Mkr7x1SystemSmokeDeviceDid111111111111111111",
  } = {},
) {
  return {
    authority: {
      signed_in: true,
      proof_binding_id: proofBindingId,
    },
    access: {
      role: "admin",
      localhost_root: "localhost://system",
      guest_registration_enabled: false,
    },
    appearance,
    identity: {
      device_did: deviceDid,
    },
    runtime: {
      version: "0.7.0-source",
    },
    source: {
      configured: true,
      name: "Source Home",
      channel: "source",
      installed_version: "0.7.0-source",
      mode: "development",
      update_policy: "manual",
      transport: "Loopback",
      source_peer: "",
      update_checks_allowed: false,
    },
  };
}

export function inertSystemApiResponse(pathname) {
  if (pathname === "/api/apps/home/active-shell") {
    return {
      active: "home-gui",
      candidates: [
        { name: "home-gui", title: "Home GUI", role: "shell", launchable: true },
        { name: "home-cli", title: "Home CLI", role: "shell", launchable: true },
      ],
    };
  }
  if (pathname === "/api/auth/passkey/status") {
    return { registered: true };
  }
  if (pathname === "/api/auth/passkeys") {
    return { passkeys: [] };
  }
  if (pathname === "/api/auth/recovery/status") {
    return {
      recovery_configured: true,
      recovery_download_available: true,
      protection_configured: true,
    };
  }
  if (pathname === "/api/capsules/catalog") {
    return { capsules: [] };
  }
  if (pathname === "/api/capsules/interfaces") {
    return { interfaces: [] };
  }
  if (pathname === "/api/provider/chain/networks") {
    return { status: "ok", data: { networks: [] } };
  }
  return null;
}

function fixtureHtml({ title, origin, background, hostScript, frameQuery = "" }) {
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>${title}</title>
    <style>
      html, body { height: 100%; margin: 0; background: ${background}; }
      iframe { width: 100%; height: 100%; border: 0; display: block; }
    </style>
    <script>
${hostScript}
    </script>
  </head>
  <body>
    <iframe
      id="system-frame"
      title="System"
      sandbox="allow-forms allow-modals allow-scripts"
      src="/apps/system/?home_origin=${encodeURIComponent(origin)}${frameQuery}#home_token=system-token"
    ></iframe>
  </body>
</html>`;
}

async function serveStaticAsset(response, url, host, fixtureOptions) {
  const pathname = url.pathname;
  if (pathname === "/favicon.ico") {
    response.writeHead(204);
    response.end();
    return;
  }
  if (pathname === "/fixture") {
    const requestedSettings = url.searchParams.get("settings") || "";
    const body = Buffer.from(
      fixtureHtml({
        title: fixtureOptions.title,
        origin: `http://${host}`,
        background: fixtureOptions.background,
        hostScript: fixtureOptions.hostScript,
        frameQuery: requestedSettings
          ? `&settings=${encodeURIComponent(requestedSettings)}`
          : "",
      }),
    );
    response.writeHead(200, {
      "cache-control": "no-store",
      "content-length": body.length,
      "content-type": "text/html; charset=utf-8",
    });
    response.end(body);
    return;
  }
  let root = null;
  let relative = "";
  if (pathname === "/apps/system/" || pathname.startsWith("/apps/system/")) {
    root = systemRoot;
    relative =
      pathname === "/apps/system/"
        ? "index.html"
        : pathname.slice("/apps/system/".length);
  } else if (
    pathname === "/apps/home/home-clipboard-client.js" ||
    pathname === "/apps/home/home-clipboard-protocol.js"
  ) {
    root = homeRoot;
    relative = pathname.slice("/apps/home/".length);
  } else if (pathname === "/apps/home-gui/wallpaper.webp") {
    root = homeGuiRoot;
    relative = "wallpaper.webp";
  }
  assert(root, "unexpected System fixture asset request", { pathname });
  const path = join(root, relative);
  assert(
    path.startsWith(`${root}/`) || path === join(root, "index.html"),
    "invalid System fixture asset path",
    { pathname, path },
  );
  const body = await readFile(path);
  const contentType =
    {
      ".css": "text/css",
      ".html": "text/html; charset=utf-8",
      ".js": "text/javascript",
      ".mjs": "text/javascript",
      ".webp": "image/webp",
      ".woff2": "font/woff2",
    }[extname(path)] || "application/octet-stream";
  response.writeHead(200, {
    "access-control-allow-origin": "null",
    "cache-control": "no-store",
    "content-length": body.length,
    "content-type": contentType,
  });
  response.end(body);
}

export async function startSystemFixtureServer({
  title,
  background = "#101216",
  hostScript,
  onApiRequest,
}) {
  const requestFailures = [];
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url ?? "/", "http://127.0.0.1");
      if (request.method === "OPTIONS") {
        response.writeHead(204, CORS_HEADERS);
        response.end();
        return;
      }
      if (
        typeof onApiRequest === "function" &&
        (await onApiRequest({ request, response, url })) === true
      ) {
        return;
      }
      const inert = inertSystemApiResponse(url.pathname);
      if (inert !== null) {
        jsonResponse(response, inert);
        return;
      }
      await serveStaticAsset(
        response,
        url,
        request.headers.host || "127.0.0.1",
        { title, background, hostScript },
      );
    } catch (error) {
      requestFailures.push(error instanceof Error ? error.message : String(error));
      textResponse(
        response,
        error instanceof Error ? error.message : String(error),
        500,
      );
    }
  });
  await new Promise((resolvePromise) =>
    server.listen(0, "127.0.0.1", resolvePromise),
  );
  const address = server.address();
  assert(address && typeof address === "object", "System fixture did not bind");
  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
    requestFailures,
    async close() {
      await new Promise((resolvePromise, rejectPromise) => {
        server.close((error) => {
          if (error) {
            rejectPromise(error);
            return;
          }
          resolvePromise();
        });
      });
    },
  };
}
