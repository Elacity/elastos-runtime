# Sysroot image for hermetic Linux producer builds (scripts/build-hermetic.py).
# Its rootfs is exported, sealed and exposed read-only as /usr, /etc, /lib,
# /bin, /sbin, /opt inside the build sandbox; the host system never is.
FROM ubuntu:24.04@sha256:534baea6a22c03a63003dbc8dbe78fe34bc0d7e595d9a9dc9834884ff530eb55
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      build-essential pkg-config libssl-dev clang cmake ca-certificates \
 && rm -rf /var/lib/apt/lists/*
