# Documents

Built-in `Documents` capsule for local-first markdown documents.

- Open it from Home to create, edit, save, and publish markdown documents.
- Provider requests are bound to the signed Home launch-token principal.
- Documents are addressed as `localhost://ElastOS/Documents/<doc-did>`.
- Working copies live under the runtime principal root: `localhost://Users/<principal-root>/Documents/...`.
- Publish and unpublish update provider-plane content availability; the capsule does not publish directly to IPFS.
