#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const DEFAULT_RELAY_SOCKET = "/tmp/elastos-browser-local-exit.sock";
const DEFAULT_ADAPTER_SOCKET = "/tmp/elastos-browser-adapter.sock";
const DEFAULT_PROFILE_ROOT = "/tmp/elastos-browser-profiles";

function usage() {
  return `Usage:
  node scripts/browser-native-operator-config.mjs \\
    --out-dir /absolute/or/relative/output-dir \\
    --browser-program /absolute/path/to/chromium-or-cef \\
    --supervisor-bin /absolute/path/to/browser-engine-supervisor \\
    --proxy-engine-bin /absolute/path/to/browser-native-proxy-engine

Options:
  --adapter-id <id>              Default: linux-native
  --engine <cef|chromium_microvm> Default: cef
  --relay-socket <path>          Default: ${DEFAULT_RELAY_SOCKET}
  --adapter-socket <path>        Default: ${DEFAULT_ADAPTER_SOCKET}
  --profile-root <path>          Default: ${DEFAULT_PROFILE_ROOT}
  --allowed-hosts <csv>          Default: *
  --allowed-schemes <csv>        Default: tcp,tls
  --allowed-ports <csv>          Default: 80,443
  --address-family <policy>      Default: prefer_ipv4
                                  system|prefer_ipv4|prefer_ipv6|ipv4_only|ipv6_only
  --allow-private-targets        Disabled by default
  --native-audio                 Declare that the native adapter has a real
                                  host audio path. Disabled by default.
  --native-video                 Declare that the native adapter has a real
                                  host video/compositor path. Disabled by default.
  --upstream-http-proxy <url>    Optional operator-approved HTTP CONNECT proxy
  --upstream-proxy-authorization <header>
                                  Optional Proxy-Authorization header value
`;
}

function parseArgs(argv) {
  const args = {
    adapterId: "linux-native",
    engine: "cef",
    relaySocket: DEFAULT_RELAY_SOCKET,
    adapterSocket: DEFAULT_ADAPTER_SOCKET,
    profileRoot: DEFAULT_PROFILE_ROOT,
    allowedHosts: ["*"],
    allowedSchemes: ["tcp", "tls"],
    allowedPorts: [80, 443],
    addressFamily: "prefer_ipv4",
    allowPrivateTargets: false,
    nativeAudio: false,
    nativeVideo: false,
    upstreamHttpProxy: "",
    upstreamProxyAuthorization: "",
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      i += 1;
      if (i >= argv.length || argv[i].startsWith("--")) {
        throw new Error(`${arg} requires a value`);
      }
      return argv[i];
    };
    switch (arg) {
      case "--help":
      case "-h":
        console.log(usage());
        process.exit(0);
      case "--out-dir":
        args.outDir = next();
        break;
      case "--browser-program":
        args.browserProgram = next();
        break;
      case "--supervisor-bin":
        args.supervisorBin = next();
        break;
      case "--proxy-engine-bin":
        args.proxyEngineBin = next();
        break;
      case "--adapter-id":
        args.adapterId = next();
        break;
      case "--engine":
        args.engine = next();
        break;
      case "--relay-socket":
        args.relaySocket = next();
        break;
      case "--adapter-socket":
        args.adapterSocket = next();
        break;
      case "--profile-root":
        args.profileRoot = next();
        break;
      case "--allowed-hosts":
        args.allowedHosts = parseCsv(next());
        break;
      case "--allowed-schemes":
        args.allowedSchemes = parseCsv(next());
        break;
      case "--allowed-ports":
        args.allowedPorts = parseCsv(next()).map((value) => {
          const port = Number(value);
          if (!Number.isInteger(port) || port < 1 || port > 65535) {
            throw new Error(`invalid allowed port: ${value}`);
          }
          return port;
        });
        break;
      case "--address-family":
        args.addressFamily = next();
        break;
      case "--allow-private-targets":
        args.allowPrivateTargets = true;
        break;
      case "--native-audio":
        args.nativeAudio = true;
        break;
      case "--native-video":
        args.nativeVideo = true;
        break;
      case "--upstream-http-proxy":
        args.upstreamHttpProxy = next();
        break;
      case "--upstream-proxy-authorization":
        args.upstreamProxyAuthorization = next();
        break;
      default:
        throw new Error(`unknown option: ${arg}`);
    }
  }
  return args;
}

function parseCsv(raw) {
  const values = raw.split(",").map((value) => value.trim()).filter(Boolean);
  if (values.length === 0) {
    throw new Error("CSV option must contain at least one value");
  }
  return values;
}

function validate(args) {
  if (!args.outDir) {
    throw new Error("--out-dir is required");
  }
  for (const [label, value] of [
    ["--browser-program", args.browserProgram],
    ["--supervisor-bin", args.supervisorBin],
    ["--proxy-engine-bin", args.proxyEngineBin],
  ]) {
    if (!value) {
      throw new Error(`${label} is required`);
    }
    validateAbsolutePath(label, value);
  }
  for (const [label, value] of [
    ["--relay-socket", args.relaySocket],
    ["--adapter-socket", args.adapterSocket],
    ["--profile-root", args.profileRoot],
  ]) {
    validateAbsolutePath(label, value);
    if (/\s|\0/.test(value)) {
      throw new Error(`${label} must not contain whitespace or NUL`);
    }
  }
  if (args.relaySocket === args.adapterSocket) {
    throw new Error("--relay-socket and --adapter-socket must differ");
  }
  if (!/^[A-Za-z0-9:_-]+$/.test(args.adapterId)) {
    throw new Error("--adapter-id must be a safe identifier");
  }
  if (!["cef", "chromium_microvm"].includes(args.engine)) {
    throw new Error("--engine must be cef or chromium_microvm");
  }
  for (const scheme of args.allowedSchemes) {
    if (!["tcp", "tls"].includes(scheme)) {
      throw new Error("--allowed-schemes may contain only tcp or tls");
    }
  }
  for (const host of args.allowedHosts) {
    if (host.length === 0 || /[\s/\\\0]/.test(host)) {
      throw new Error(`invalid allowed host: ${host}`);
    }
  }
  if (!["system", "prefer_ipv4", "prefer_ipv6", "ipv4_only", "ipv6_only"].includes(args.addressFamily)) {
    throw new Error("--address-family must be system, prefer_ipv4, prefer_ipv6, ipv4_only, or ipv6_only");
  }
  if (args.upstreamHttpProxy) {
    const proxy = new URL(args.upstreamHttpProxy);
    if (proxy.protocol !== "http:") {
      throw new Error("--upstream-http-proxy must use http://");
    }
    if (!proxy.hostname || !proxy.port) {
      throw new Error("--upstream-http-proxy must include host and explicit port");
    }
    if (proxy.username || proxy.password) {
      throw new Error("--upstream-http-proxy credentials must use --upstream-proxy-authorization");
    }
  }
  if (/[\r\n\0]/.test(args.upstreamProxyAuthorization)) {
    throw new Error("--upstream-proxy-authorization must not contain CR, LF, or NUL");
  }
}

function validateAbsolutePath(label, value) {
  if (typeof value !== "string" || !value.startsWith("/")) {
    throw new Error(`${label} must be an absolute path`);
  }
}

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function buildConfigs(args) {
  const nativeProxyConfig = {
    schema: "elastos.browser.native-proxy-engine.config/v1",
    browser_program: args.browserProgram,
    browser_args: [
      "--proxy-server={proxy_url}",
      "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1",
      "--disable-background-networking",
      "--no-first-run",
      `--user-data-dir=${args.profileRoot}/{stream_id}`,
      "{url}",
    ],
    startup_grace_ms: 1000,
  };

  const supervisorConfig = {
    schema: "elastos.browser.engine.supervisor-config/v1",
    adapter: args.adapterId,
    engine: args.engine,
    program: args.proxyEngineBin,
    args: [],
    env: {
      ELASTOS_BROWSER_NATIVE_PROXY_ENGINE_CONFIG: JSON.stringify(nativeProxyConfig),
    },
    network_sandbox: "linux_new_netns",
    startup_grace_ms: 1000,
    display_capabilities: {
      audio: args.nativeAudio,
      video: args.nativeVideo,
    },
  };

  const browserEngineAdapter = {
    adapters: [
      {
        id: args.adapterId,
        kind: args.engine,
        network_mode: "runtime_net_only",
        display_modes: ["native_surface"],
        supervisor: {
          program: args.supervisorBin,
          args: [],
          env: {
            ELASTOS_BROWSER_ENGINE_SUPERVISOR_CONFIG: JSON.stringify(supervisorConfig),
          },
          timeout_ms: 30000,
        },
      },
    ],
  };

  const exitProvider = {
    timeout_secs: 10,
    backends: [
      {
        id: "local-runtime-browser-exit",
        kind: "stream_relay",
        allowed_hosts: args.allowedHosts,
        allowed_schemes: args.allowedSchemes,
        allowed_ports: args.allowedPorts,
        allow_private_targets: args.allowPrivateTargets,
        adapter_ipc: {
          kind: "unix_socket",
          path: args.adapterSocket,
        },
        relay_ipc: {
          kind: "unix_socket",
          path: args.relaySocket,
        },
      },
    ],
  };

  const browserLocalExit = {
    schema: "elastos.browser.local-exit.config/v1",
    relay_ipc_path: args.relaySocket,
    allowed_hosts: args.allowedHosts,
    allowed_schemes: args.allowedSchemes,
    allowed_ports: args.allowedPorts,
    address_family: args.addressFamily,
    allow_private_targets: args.allowPrivateTargets,
    replace_existing_socket: true,
  };
  if (args.upstreamHttpProxy) {
    browserLocalExit.upstream_http_proxy = {
      url: args.upstreamHttpProxy,
    };
    if (args.upstreamProxyAuthorization) {
      browserLocalExit.upstream_http_proxy.authorization_header = args.upstreamProxyAuthorization;
    }
  }

  return {
    browserEngineAdapter,
    exitProvider,
    browserLocalExit,
    nativeProxyConfig,
    supervisorConfig,
  };
}

function validateGenerated(configs) {
  const adapter = configs.browserEngineAdapter.adapters[0];
  const supervisorRaw = adapter.supervisor.env.ELASTOS_BROWSER_ENGINE_SUPERVISOR_CONFIG;
  const supervisor = JSON.parse(supervisorRaw);
  const nativeRaw = supervisor.env.ELASTOS_BROWSER_NATIVE_PROXY_ENGINE_CONFIG;
  const native = JSON.parse(nativeRaw);

  if (supervisor.adapter !== adapter.id) {
    throw new Error("generated supervisor adapter does not match browser-engine-adapter id");
  }
  if (supervisor.engine !== adapter.kind) {
    throw new Error("generated supervisor engine does not match browser-engine-adapter kind");
  }
  if (native.schema !== "elastos.browser.native-proxy-engine.config/v1") {
    throw new Error("generated native proxy config has wrong schema");
  }
  if (configs.exitProvider.backends[0].relay_ipc.path !== configs.browserLocalExit.relay_ipc_path) {
    throw new Error("generated exit-provider relay_ipc path does not match browser-local-exit");
  }
  if (configs.browserLocalExit.upstream_http_proxy) {
    const proxy = new URL(configs.browserLocalExit.upstream_http_proxy.url);
    if (proxy.protocol !== "http:") {
      throw new Error("generated upstream proxy must be http");
    }
  }
  const displayCapabilities = supervisor.display_capabilities;
  if (!displayCapabilities || typeof displayCapabilities.audio !== "boolean" || typeof displayCapabilities.video !== "boolean") {
    throw new Error("generated supervisor config must explicitly declare native display capabilities");
  }
}

function main() {
  try {
    const args = parseArgs(process.argv.slice(2));
    validate(args);
    const configs = buildConfigs(args);
    validateGenerated(configs);

    const outDir = path.resolve(args.outDir);
    fs.mkdirSync(outDir, { recursive: true });
    writeJson(path.join(outDir, "browser-engine-adapter.json"), configs.browserEngineAdapter);
    writeJson(path.join(outDir, "exit-provider.json"), configs.exitProvider);
    writeJson(path.join(outDir, "browser-local-exit.json"), configs.browserLocalExit);

    console.log(JSON.stringify({
      ok: true,
      out_dir: outDir,
      files: [
        "browser-engine-adapter.json",
        "exit-provider.json",
        "browser-local-exit.json",
      ],
      adapter_id: args.adapterId,
      engine: args.engine,
      relay_socket: args.relaySocket,
      adapter_socket: args.adapterSocket,
      network_mode: "runtime_net_only",
      direct_network: false,
      wallet_injection: false,
      native_audio_declared: args.nativeAudio,
      native_video_declared: args.nativeVideo,
      address_family: args.addressFamily,
      upstream_http_proxy: Boolean(args.upstreamHttpProxy),
    }));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    console.error(usage());
    process.exit(1);
  }
}

main();
