# Documents

`Documents` is the built-in capsule for local-first Markdown documents.

- Open it from Home to create, edit and save Markdown and text documents. To
  share a file publicly, publish it from Library.
- Runtime binds provider requests to the signed Home launch-token principal.
- Documents are addressed as `localhost://ElastOS/Documents/<doc-did>`.
- Working copies live under the Runtime principal root:
  `localhost://Users/<principal-root>/Documents/...`.
- Unpublish withdraws an earlier publication through the provider. The capsule
  does not publish directly to IPFS.
