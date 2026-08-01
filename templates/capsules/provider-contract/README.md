# Provider contract template

This manifest is not a working provider. Before activation, add:

- a Runtime-owned provider implementation and registration
- canonical operation-to-action mappings
- capability upper-bound and denial tests
- request and completion audit tests
- lifecycle, capacity, cleanup, and cross-platform proof
- Carrier routing proof when Runtime selects Carrier for a remote peer or
  content effect

Never place raw provider credentials, sockets, host paths, or backend URLs in an
app capsule or public catalog projection.
