# syntax=docker/dockerfile:1

FROM ghcr.io/selkies-project/selkies/py-build:main AS selkies-wheel

FROM ghcr.io/selkies-project/selkies-gstreamer/gst-py-example:main-ubuntu24.04

ENV DEBIAN_FRONTEND=noninteractive
ARG PIXELFLUX_VERSION=1.4.7

RUN sudo-root sh -c 'grep -rl "launchpadcontent.net" /etc/apt/sources.list /etc/apt/sources.list.d 2>/dev/null | xargs -r rm -f' \
  && sudo-root apt-get update \
  && sudo-root apt-get install --no-install-recommends -y \
    libnspr4 \
    libnss3 \
    libxkbcommon-dev \
    xclip \
  && sudo-root rm -rf /var/lib/apt/lists/*

COPY --from=selkies-wheel /opt/pypi/dist/selkies-0.0.0.dev0-py3-none-any.whl /tmp/selkies-0.0.0.dev0-py3-none-any.whl

RUN python3 -m pip install --break-system-packages --no-cache-dir /tmp/selkies-0.0.0.dev0-py3-none-any.whl \
  && python3 -m pip install --break-system-packages --no-cache-dir --force-reinstall "pixelflux==${PIXELFLUX_VERSION}" \
  && python3 -c 'import ctypes, os, pixelflux; lib = os.path.join(os.path.dirname(pixelflux.__file__), "screen_capture_module.so"); ctypes.CDLL(lib)' \
  && rm -f /tmp/selkies-0.0.0.dev0-py3-none-any.whl
