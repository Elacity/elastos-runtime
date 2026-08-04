# Documents

`Documents` is the built-in capsule for local-first Markdown documents.

- Open it from Home to create, edit, save, and publish Markdown documents.
- Runtime binds provider requests to the signed Home launch-token principal.
- Documents are addressed as `localhost://ElastOS/Documents/<doc-did>`.
- Working copies live under the Runtime principal root:
  `localhost://Users/<principal-root>/Documents/...`.
- Publish and unpublish update content availability through the provider. The
  capsule does not publish directly to IPFS.
