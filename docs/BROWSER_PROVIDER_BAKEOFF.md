# Browser provider research

The Browser Engine contract is defined in [BROWSER_CAPSULE.md](BROWSER_CAPSULE.md).
One Chromium + Selkies + Runtime network-wrapper recipe runs in a per-launch
Linux guest. Mac Apple Virtualization and Linux crosvm are host adapters for
that Engine. A host without compatible virtualization selects an approved
remote Engine through Runtime.

Earlier hosted-provider comparisons are historical research. Kasm Workspaces,
BrowserBox and host-native namespace launchers are outside normal Browser
delivery. Runtime owns Browser grants and lifecycle; Engine executes websites;
Exit owns authorized website connections; Carrier carries off-box traffic.

Product acceptance uses target machine evidence and matching human evidence.
Source checks alone establish source behavior. The objective audit retains the
existing media, input, profile, cleanup and hash-bound manual UX requirements:

```bash
node scripts/browser-objective-audit.mjs \
  --vm-proof /path/to/mac-vm-proof.json \
  --manual-ux /path/to/manual-ux.json
```

The Mac proof uses `elastos.browser.mac-vm-proof/v1` and the existing detailed
VM validator. Jetson target qualification remains an independent hardware gate;
a Mac receipt establishes only the Mac result. Wallet, media, network, soak,
second-maintainer and human approval gates remain with their owning issues.
