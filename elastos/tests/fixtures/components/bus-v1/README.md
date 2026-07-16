# ElastOS Bus v1 Conformance Fixture

This directory is test input, not an installable product capsule. The server
test launches the checked-in Component through Runtime, grants its pending
capability through the real capability manager, dispatches its request to a
registered test provider, and verifies Runtime-owned audit events.

Run `./build.sh` from this directory to reproduce the Component artifact. The
fixture intentionally imports only the checked-in `elastos:bus@v1` world. It
has no WASI, environment, preopen, socket, FIFO, or HTTP authority.
