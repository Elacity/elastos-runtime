#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APPKIT_VERSION="${APPKIT_VERSION:-1.8.19}"
WAGMI_VERSION="${WAGMI_VERSION:-3.6.10}"
VIEM_VERSION="${VIEM_VERSION:-2.48.11}"
ESBUILD_VERSION="${ESBUILD_VERSION:-0.28.0}"
WALLETCONNECT_ETHEREUM_PROVIDER_VERSION="${WALLETCONNECT_ETHEREUM_PROVIDER_VERSION:-2.23.9}"
COINBASE_WALLET_SDK_VERSION="${COINBASE_WALLET_SDK_VERSION:-4.3.7}"
METAMASK_CONNECT_EVM_VERSION="${METAMASK_CONNECT_EVM_VERSION:-1.1.0}"
PORTO_VERSION="${PORTO_VERSION:-0.2.37}"
OUT_FILE="$ROOT_DIR/artifacts/walletconnect/reown-appkit.js"

usage() {
  cat <<'EOF'
Usage:
  bash scripts/vendor-walletconnect-adapter.sh [--out-file <path>]

Environment overrides:
  APPKIT_VERSION   exact @reown/appkit and @reown/appkit-adapter-wagmi version
  WAGMI_VERSION    exact wagmi version
  VIEM_VERSION     exact viem version
  ESBUILD_VERSION  exact esbuild version
  WALLETCONNECT_ETHEREUM_PROVIDER_VERSION exact WalletConnect EIP-1193 provider version
  COINBASE_WALLET_SDK_VERSION             exact Coinbase connector dependency version
  METAMASK_CONNECT_EVM_VERSION            exact MetaMask connector dependency version
  PORTO_VERSION                           exact Porto connector dependency version

The output is a local ESM adapter bundle exporting connectWalletConnectEvm().
Review it, then pin it with:

  node scripts/configure-walletconnect-connector.mjs \
    --data-dir <runtime-data-dir> \
    --project-id <reown-project-id> \
    --sdk-file <adapter-bundle> \
    --sdk-version <exact-version>
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out-file)
      OUT_FILE="${2:?missing --out-file value}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

for version in \
  "$APPKIT_VERSION" \
  "$WAGMI_VERSION" \
  "$VIEM_VERSION" \
  "$ESBUILD_VERSION" \
  "$WALLETCONNECT_ETHEREUM_PROVIDER_VERSION" \
  "$COINBASE_WALLET_SDK_VERSION" \
  "$METAMASK_CONNECT_EVM_VERSION" \
  "$PORTO_VERSION"; do
  if [[ -z "$version" || "$version" == ^* || "$version" == ~* || "$version" == "*" || "$version" =~ [[:space:]] ]]; then
    echo "All package versions must be exact pins. Invalid: $version" >&2
    exit 1
  fi
done

tmpdir="$(mktemp -d /tmp/elastos-walletconnect-adapter-XXXXXX)"
trap 'rm -rf "$tmpdir"' EXIT

cat >"$tmpdir/package.json" <<EOF
{
  "private": true,
  "type": "module",
  "dependencies": {
    "@reown/appkit": "$APPKIT_VERSION",
    "@reown/appkit-adapter-wagmi": "$APPKIT_VERSION",
    "wagmi": "$WAGMI_VERSION",
    "viem": "$VIEM_VERSION",
    "esbuild": "$ESBUILD_VERSION",
    "@walletconnect/ethereum-provider": "$WALLETCONNECT_ETHEREUM_PROVIDER_VERSION",
    "@coinbase/wallet-sdk": "$COINBASE_WALLET_SDK_VERSION",
    "@metamask/connect-evm": "$METAMASK_CONNECT_EVM_VERSION",
    "porto": "$PORTO_VERSION"
  }
}
EOF

cat >"$tmpdir/adapter.js" <<'JS'
import { createAppKit } from "@reown/appkit";
import { base, defineChain, mainnet } from "@reown/appkit/networks";
import { WagmiAdapter } from "@reown/appkit-adapter-wagmi";

const elastosEsc = defineChain({
  id: 20,
  caipNetworkId: "eip155:20",
  chainNamespace: "eip155",
  name: "Elastos Smart Chain",
  nativeCurrency: {
    decimals: 18,
    name: "ELA",
    symbol: "ELA",
  },
  rpcUrls: {
    default: { http: ["https://api.elastos.io/esc"] },
  },
  blockExplorers: {
    default: { name: "Elastos Explorer", url: "https://esc.elastos.io" },
  },
});

const supportedNetworks = new Map([
  [1, mainnet],
  [20, elastosEsc],
  [8453, base],
]);

let modal = null;
let adapter = null;
let modalKey = "";

export async function connectWalletConnectEvm(options = {}) {
  const projectId = requiredText(options.projectId, "projectId");
  const chains = Array.isArray(options.chains) && options.chains.length > 0
    ? options.chains
    : [1, 20, 8453];
  const networks = chains.map((chainId) => supportedNetworks.get(Number(chainId))).filter(Boolean);
  if (networks.length === 0) {
    throw new Error("No supported EVM networks configured for WalletConnect.");
  }

  const key = `${projectId}:${networks.map((network) => network.id).join(",")}`;
  if (!modal || modalKey !== key) {
    adapter = new WagmiAdapter({ projectId, networks });
    modal = createAppKit({
      adapters: [adapter],
      networks,
      projectId,
      metadata: options.metadata || {},
      enableReconnect: false,
      enableWalletGuide: false,
      features: {
        analytics: false,
        email: false,
        socials: [],
        swaps: false,
        onramp: false,
      },
    });
    modalKey = key;
  }

  const existingProvider = eip1193Provider(modal.getWalletProvider?.());
  if (existingProvider && modal.getIsConnected?.()) {
    return existingProvider;
  }

  const providerPromise = waitForProvider(modal);
  await modal.open({ view: "Connect", namespace: "eip155" });
  return providerPromise;
}

function waitForProvider(currentModal) {
  return new Promise((resolve, reject) => {
    let done = false;
    let unsubscribe = null;
    const timeout = setTimeout(() => {
      finish(null, new Error("WalletConnect connection was not completed."));
    }, 300000);

    function finish(provider, error) {
      if (done) {
        return;
      }
      done = true;
      clearTimeout(timeout);
      if (typeof unsubscribe === "function") {
        unsubscribe();
      }
      if (error) {
        reject(error);
      } else {
        resolve(provider);
      }
    }

    unsubscribe = currentModal.subscribeProvider((state) => {
      if (state?.error) {
        finish(null, state.error);
        return;
      }
      const provider = eip1193Provider(state?.provider);
      if (provider && state?.isConnected) {
        finish(provider, null);
      }
    });

    const provider = eip1193Provider(currentModal.getWalletProvider?.());
    if (provider && currentModal.getIsConnected?.()) {
      finish(provider, null);
    }
  });
}

function eip1193Provider(value) {
  return value && typeof value.request === "function" ? value : null;
}

function requiredText(value, label) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`Missing ${label}.`);
  }
  return value.trim();
}
JS

(
  cd "$tmpdir"
  npm install --ignore-scripts --no-audit --no-fund >/dev/null
  npx esbuild adapter.js \
    --bundle \
    --format=esm \
    --platform=browser \
    --target=es2022 \
    --outfile="$OUT_FILE" >/dev/null
)

sha="$(sha256sum "$OUT_FILE" | awk '{print $1}')"
cat <<EOF
WalletConnect adapter bundle written:
  path: $OUT_FILE
  sha256: $sha
  appkit: $APPKIT_VERSION
  wagmi: $WAGMI_VERSION
  viem: $VIEM_VERSION
  esbuild: $ESBUILD_VERSION
  walletconnect_ethereum_provider: $WALLETCONNECT_ETHEREUM_PROVIDER_VERSION
  coinbase_wallet_sdk: $COINBASE_WALLET_SDK_VERSION
  metamask_connect_evm: $METAMASK_CONNECT_EVM_VERSION
  porto: $PORTO_VERSION
EOF
