# Viewer and content pair

Copy `viewer.capsule.json` to `capsules/<viewer>/capsule.json` and copy the
`browser/` directory with it. Copy `content.capsule.json` and `sample.example`
to a separate content capsule.

Rename `example-viewer` and `example-content` throughout both manifests. The
content manifest's `viewer` and `output_schema.viewer` fields must match the
viewer manifest's `content_capsule` acceptance binding. Keep the content
entrypoint extension and `content_type` consistent with the viewer's file
acceptance.
