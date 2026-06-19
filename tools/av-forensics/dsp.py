#!/usr/bin/env python3
"""Video DSP for the offline forensic extractor (vendored from the proven Phase-0/5 spike).

The validated mark: a per-variant, antipodal, mid-band spread-spectrum luma carrier, activity-masked
so it hides in texture. Detection is differential & informed (distributor holds the master):
optionally FM-align a leaked frame to the master, then correlate the residual with the carrier.

Requires numpy + a system `ffmpeg`. This is the operator forensic tool, not a hot-path module.
The codeword math it feeds is in `canonical.py` (welded to the Rust serve selector).
"""
import subprocess as sp

import numpy as np

W, H, FPS, N = 480, 270, 30, 120
SEG = 8                       # frames per segment (real CMAF segments are 8-22x longer)
ALPHA = 3.2                   # embed strength (high-PSNR, imperceptible mark)
R_LO, R_HI = 0.12, 0.32       # mid-frequency annulus (fraction of Nyquist), the masked band


def run(cmd):
    sp.run(cmd, check=True, stdout=sp.DEVNULL, stderr=sp.DEVNULL)


def load_gray(path, n=N):
    a = np.fromfile(path, dtype=np.uint8)
    return a[: n * H * W].reshape(-1, H, W).astype(np.float64)


def bandpass_pattern(seed):
    """Zero-mean, unit-variance mid-frequency carrier (noise, annulus-filtered)."""
    rng = np.random.default_rng(seed)
    F = np.fft.fft2(rng.standard_normal((H, W)))
    fy = np.fft.fftfreq(H)[:, None] * 2
    fx = np.fft.fftfreq(W)[None, :] * 2
    r = np.sqrt(fy ** 2 + fx ** 2)
    F *= (r >= R_LO) & (r <= R_HI)
    w = np.real(np.fft.ifft2(F))
    w -= w.mean()
    w /= w.std() + 1e-9
    return w


W_PAT = bandpass_pattern(7)


def box_mean(img, k=9):
    pad = k // 2
    p = np.pad(img, pad, mode="reflect")
    ii = p.cumsum(0).cumsum(1)
    ii = np.pad(ii, ((1, 0), (1, 0)))
    s = ii[k:, k:] - ii[:-k, k:] - ii[k:, :-k] + ii[:-k, :-k]
    return s / (k * k)


def activity_mask(img):
    """Normalized local std in [0,1] — concentrate the mark where texture hides it."""
    m = box_mean(img)
    ms = box_mean(img * img)
    std = np.sqrt(np.maximum(ms - m * m, 0))
    return np.clip(std / 16.0, 0.0, 1.0)


def embed(frames, frame_bits):
    """Embed a per-frame antipodal bit (+1/-1) as the activity-masked carrier."""
    out = np.empty_like(frames)
    for i, f in enumerate(frames):
        out[i] = np.clip(f + ALPHA * frame_bits[i] * W_PAT * activity_mask(f), 0, 255)
    return np.round(out).astype(np.uint8)


def encode(frames, path, crf=23, vcodec="libx264"):
    raw = path + ".gray"
    frames.astype(np.uint8).tofile(raw)
    run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt",
         "gray", "-s", f"{W}x{H}", "-r", str(FPS), "-i", raw, "-c:v", vcodec, "-crf", str(crf),
         "-pix_fmt", "yuv420p", path])


def decode(path):
    raw = path + ".dec.gray"
    run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", path, "-vf", f"scale={W}:{H}",
         "-pix_fmt", "gray", "-f", "rawvideo", raw])
    return load_gray(raw)


def ff_transform(src, name):
    """Produce a leaked file via an adversary transform; return its path."""
    o = f"leak_{name}.mp4"
    vf = None
    if name == "crop10":
        vf = f"crop={int(W * 0.9) // 2 * 2}:{int(H * 0.9) // 2 * 2},scale={W}:{H}"
    elif name == "crop20":
        vf = f"crop={int(W * 0.8) // 2 * 2}:{int(H * 0.8) // 2 * 2},scale={W}:{H}"
    elif name == "zoom_shift":
        vf = f"crop={int(W * 0.85) // 2 * 2}:{int(H * 0.85) // 2 * 2}:20:12,scale={W}:{H}"
    elif name != "reencode":
        raise ValueError(f"unknown transform {name!r}")
    cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", src]
    if vf:
        cmd += ["-vf", vf]
    cmd += ["-c:v", "libx264", "-crf", "26", o]
    run(cmd)
    return o


def align_with(frame, est):
    from fm_register import warp
    rs = warp(frame, 1.0 / est["scale"], -est["angle"], 0, 0)
    return warp(rs, 1.0, 0.0, est["tx"], est["ty"])


def detect(frames, master, est=None):
    """Differential detect: (optionally FM-align) each frame to master, correlate residual with the
    carrier. Returns per-frame z (true corr / null std). `est=None` ⇒ no registration (baseline)."""
    z = np.empty(len(frames))
    for i, f in enumerate(frames):
        m = master[i]
        a = align_with(f, est) if est else f
        d = a - m
        vm = (a != 0.0) if est else np.ones_like(a, bool)   # exclude lost-border pixels
        c = float((d * W_PAT * vm).sum())
        null = np.array([float((d * np.roll(W_PAT, 37 * (k + 1)) * vm).sum()) for k in range(12)])
        z[i] = c / (null.std() + 1e-9)
    return z
