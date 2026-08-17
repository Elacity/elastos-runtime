#!/usr/bin/env node
/* verify-selectors.mjs — empirically confirm the marketplace contract selectors against the
 * DEPLOYED Base-mainnet bytecode (resolving the EIP-1967 proxy implementation).
 *
 * Why: keccak-correct ≠ ABI-correct. This resolves CONTRACTS.md's "to-verify" gap by checking the
 * 4-byte selectors are actually present in the deployed implementation. Re-run after any proxy
 * UPGRADE (the impl address can change). Read-only (eth_getStorageAt + eth_getCode); no keys.
 *
 * Usage: node verify-selectors.mjs [gatewayAddress]
 * Default gateway = the AuthorityGateway the runtime pins (buy_authority.rs:58).
 */
const GW = process.argv[2] || "0x09dBe796f40ECEffEAccf243c3d758C4c1d8D87D";
const EIP1967_IMPL = "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc";
const RPCS = ["https://mainnet.base.org", "https://base.llamarpc.com", "https://base-rpc.publicnode.com"];

// selector (4-byte, no 0x) -> signature it should be
const SELECTORS = {
  f7580ad9: "buyAccess (native)",
  "0ede2294": "buyAccess (ERC-20)",
  "54d42821": "hasAccessByContentId(address,bytes16)",
  "9a3fa9f5": "sellAccess / list",
  "3e65bbba": "withdrawListing / cancel",
  f1c6bdf8: "paymentProcessor()  [ERC-20 approve target — buy critical path]",
  "997eab2d": "sellersOf",
  "6bd3a64b": "listings",
};

async function rpc(method, params) {
  for (const url of RPCS) {
    try {
      const r = await fetch(url, { method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) });
      const j = await r.json();
      if (j.result !== undefined) return j.result;
    } catch { /* try next */ }
  }
  return null;
}

(async () => {
  console.log(`gateway (proxy): ${GW}`);
  const slot = await rpc("eth_getStorageAt", [GW, EIP1967_IMPL, "latest"]);
  if (slot === null) { console.log("no RPC reachable — run from a network that can hit a Base RPC."); process.exit(2); }
  const impl = slot && slot !== "0x" + "0".repeat(64) ? "0x" + slot.slice(26) : null;
  const target = impl || GW;
  console.log(impl ? `implementation (EIP-1967): ${impl}` : "not an EIP-1967 proxy — checking the address directly");
  const code = await rpc("eth_getCode", [target, "latest"]);
  if (!code || code.length <= 4) { console.log("no bytecode at target."); process.exit(2); }
  console.log(`bytecode: ${(code.length - 2) / 2} bytes\n`);
  let missing = 0;
  for (const [sel, sig] of Object.entries(SELECTORS)) {
    const present = code.includes(sel);
    if (!present) missing++;
    console.log(`  ${present ? "✓" : "✗ ABSENT"}  0x${sel}  ${sig}`);
  }
  console.log(`\n${missing === 0 ? "ALL PRESENT — selectors confirmed against deployed bytecode." : missing + " ABSENT — investigate (wrong signature, different contract, or post-upgrade drift)."}`);
  process.exit(missing === 0 ? 0 : 1);
})();
