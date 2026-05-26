#!/usr/bin/env node
import fs from "node:fs";
import http from "node:http";
import process from "node:process";

const SCHEMA = "elastos.browser.selkies-close-page/v1";

function usage() {
  console.error(`Usage:
  node scripts/browser-selkies-close-page.mjs \\
    --control-socket /path/to/selkies-control.sock \\
    --page-id page:selkies-... \\
    [--confirm-close]

By default this is a dry run and prints the exact confirmed command. It mutates
the live Selkies session only when --confirm-close is present.
`);
}

function fail(message) {
  console.error(message);
  process.exit(1);
}

function parseArgs(argv) {
  const args = {
    controlSocket: "",
    pageId: "",
    confirmClose: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = () => {
      index += 1;
      if (index >= argv.length || argv[index].startsWith("--")) {
        fail(`${arg} requires a value`);
      }
      return argv[index];
    };
    if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    } else if (arg === "--control-socket") {
      args.controlSocket = next();
    } else if (arg === "--page-id") {
      args.pageId = next();
    } else if (arg === "--confirm-close") {
      args.confirmClose = true;
    } else {
      fail(`unknown option: ${arg}`);
    }
  }
  if (!args.controlSocket.startsWith("/") || /[\s\0]/.test(args.controlSocket)) {
    fail("--control-socket must be an absolute Unix socket path without whitespace");
  }
  if (!/^page:[A-Za-z0-9:_-]+$/.test(args.pageId)) {
    fail("--page-id must be a safe Browser page id");
  }
  return args;
}

function isSocket(path) {
  try {
    return fs.statSync(path).isSocket();
  } catch {
    return false;
  }
}

function closePage(controlSocket, pageId) {
  return new Promise((resolve, reject) => {
    const request = http.request(
      {
        socketPath: controlSocket,
        method: "POST",
        path: `/pages/${encodeURIComponent(pageId)}/close`,
        headers: {
          accept: "application/json",
          "content-length": "2",
          "content-type": "application/json",
        },
      },
      (response) => {
        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => {
          const text = Buffer.concat(chunks).toString("utf8");
          let body = {};
          if (text) {
            try {
              body = JSON.parse(text);
            } catch {
              return reject(new Error(`Selkies close returned non-JSON response: ${text}`));
            }
          }
          if (response.statusCode < 200 || response.statusCode >= 300) {
            return reject(new Error(body.error || body.message || `Selkies close failed: HTTP ${response.statusCode}`));
          }
          resolve({ statusCode: response.statusCode, body });
        });
      },
    );
    request.on("error", reject);
    request.end("{}");
  });
}

function shellQuote(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const confirmCommand = [
    "node",
    "scripts/browser-selkies-close-page.mjs",
    "--control-socket",
    shellQuote(args.controlSocket),
    "--page-id",
    shellQuote(args.pageId),
    "--confirm-close",
  ].join(" ");
  if (!args.confirmClose) {
    console.log(JSON.stringify({
      schema: SCHEMA,
      ok: true,
      dry_run: true,
      would_close: true,
      page_id: args.pageId,
      control_socket: args.controlSocket,
      confirm_command: confirmCommand,
    }, null, 2));
    return;
  }
  if (!isSocket(args.controlSocket)) {
    fail(`control socket is not available: ${args.controlSocket}`);
  }
  const result = await closePage(args.controlSocket, args.pageId);
  const closed = result.body?.closed === true && result.body?.page_id === args.pageId;
  console.log(JSON.stringify({
    schema: SCHEMA,
    ok: closed,
    dry_run: false,
    page_id: args.pageId,
    control_socket: args.controlSocket,
    response_status: result.statusCode,
    response: result.body,
  }, null, 2));
  if (!closed) {
    process.exit(1);
  }
}

main().catch((error) => fail(error instanceof Error ? error.message : String(error)));
