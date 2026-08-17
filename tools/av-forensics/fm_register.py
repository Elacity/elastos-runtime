#!/usr/bin/env python3
"""Deterministic geometric registration (Fourier-Mellin), INFORMED forensics.

The distributor holds the master frame, so registration is image-to-reference (a solved problem),
NOT the blind self-sync that doomed brute search. FM recovers (scale, rotation) from the log-polar
of the FFT magnitude (translation-invariant); phase-correlation recovers translation. Deterministic
— reads peak positions, no search.

LOAD-BEARING FIX (proven by the end-to-end extractor, do not regress): the FFT magnitude is
point-symmetric, so scale-direction / rotation-sign are inherently ambiguous. We resolve them by
lowest re-alignment residual against the master — **computed on the VALID (non-border) region only**.
Whole-frame residual lets lost-border zeros from a wrong scale-direction win, mis-resolving e.g. a
10% crop to scale 0.89 and sending recovery to garbage. Requires numpy.
"""
import numpy as np


def bilinear(img, yi, xi):
    H, W = img.shape
    x0 = np.floor(xi).astype(int)
    y0 = np.floor(yi).astype(int)
    x1 = x0 + 1
    y1 = y0 + 1
    inb = (x0 >= 0) & (x1 < W) & (y0 >= 0) & (y1 < H)
    x0c, x1c = np.clip(x0, 0, W - 1), np.clip(x1, 0, W - 1)
    y0c, y1c = np.clip(y0, 0, H - 1), np.clip(y1, 0, H - 1)
    wx = xi - x0
    wy = yi - y0
    v = (img[y0c, x0c] * (1 - wx) * (1 - wy) + img[y0c, x1c] * wx * (1 - wy)
         + img[y1c, x0c] * (1 - wx) * wy + img[y1c, x1c] * wx * wy)
    return np.where(inb, v, 0.0)


def warp(img, scale, angle_deg, tx, ty):
    """Apply (scale about center, rotate, translate) to content via inverse sampling."""
    H, W = img.shape
    cy, cx = (H - 1) / 2, (W - 1) / 2
    th = np.deg2rad(angle_deg)
    c, s = np.cos(th), np.sin(th)
    ys, xs = np.mgrid[0:H, 0:W].astype(float)
    x = xs - cx - tx
    y = ys - cy - ty
    xi = (c * x + s * y) / scale + cx
    yi = (-s * x + c * y) / scale + cy
    return bilinear(img, yi, xi)


def _hann2d(H, W):
    return np.outer(np.hanning(H), np.hanning(W))


def _logpolar(mag, n_rho=256, n_theta=256):
    H, W = mag.shape
    cy, cx = H / 2.0, W / 2.0
    r_max = min(H, W) / 2.0 - 1
    log_base = np.log(r_max) / n_rho
    rho = np.exp(np.arange(n_rho) * log_base)            # 1 .. r_max
    theta = np.arange(n_theta) * np.pi / n_theta         # 0 .. pi (FFT-mag is point-symmetric)
    R, T = np.meshgrid(rho, theta, indexing="ij")
    xi = cx + R * np.cos(T)
    yi = cy + R * np.sin(T)
    return bilinear(mag, yi, xi), log_base


def _phase_corr(a, b):
    """Return signed (dy,dx) of the shift aligning b onto a, plus peak height."""
    A = np.fft.fft2(a)
    B = np.fft.fft2(b)
    R = A * np.conj(B)
    R /= np.abs(R) + 1e-9
    r = np.fft.ifft2(R).real
    k = int(np.argmax(r))
    peak = r.flat[k]
    dy, dx = divmod(k, a.shape[1])
    if dy > a.shape[0] // 2:
        dy -= a.shape[0]
    if dx > a.shape[1] // 2:
        dx -= a.shape[1]
    return dy, dx, float(peak)


def _boxsmooth(a, k=25):
    pad = k // 2
    p = np.pad(a, pad, mode="reflect")
    ii = np.pad(p.cumsum(0).cumsum(1), ((1, 0), (1, 0)))
    s = ii[k:, k:] - ii[:-k, k:] - ii[k:, :-k] + ii[:-k, :-k]
    return s / (k * k)


def estimate_scale_rotation(moving, fixed):
    """FM core: recover (scale, angle_deg) mapping `moving` content to `fixed`. WHITENS the
    magnitude (log + local-mean subtraction) so no dominant orientation biases the rotation peak."""
    win = _hann2d(*fixed.shape)
    Mf = np.fft.fftshift(np.abs(np.fft.fft2(fixed * win)))
    Mm = np.fft.fftshift(np.abs(np.fft.fft2(moving * win)))
    Lf = np.log1p(Mf)
    Lm = np.log1p(Mm)
    Lf = Lf - _boxsmooth(Lf)
    Lm = Lm - _boxsmooth(Lm)
    LPf, log_base = _logpolar(Lf)
    LPm, _ = _logpolar(Lm)
    drho, dtheta, _ = _phase_corr(LPf, LPm)
    scale = float(np.exp(drho * log_base))
    angle = float(dtheta * 180.0 / LPf.shape[1])
    return scale, angle


def register(moving, fixed):
    """Full FM register. Resolves the FM sign/direction ambiguity by picking the candidate whose
    re-alignment to the reference has the lowest residual ON THE VALID REGION (4 candidates, no
    search)."""
    smag, amag = estimate_scale_rotation(moving, fixed)
    best = None
    for s_try in (smag, 1.0 / smag):
        for a_try in (amag, -amag):
            rs = warp(moving, 1.0 / s_try, -a_try, 0, 0)     # undo scale+rotation
            dy, dx, _ = _phase_corr(fixed, rs)
            aligned = warp(rs, 1.0, 0.0, dx, dy)             # undo translation
            # Resolve on the VALID region only — lost-border zeros from a wrong scale-direction
            # otherwise pollute a whole-frame residual and mispick (the load-bearing fix).
            vm = aligned != 0.0
            res = float(np.abs((aligned - fixed)[vm]).mean()) if vm.sum() > vm.size // 4 else 1e18
            if best is None or res < best[0]:
                best = (res, aligned, dict(scale=s_try, angle=a_try, ty=dy, tx=dx))
    return best[1], best[2]


# ---- oracle self-test: recover a KNOWN transform on a synthetic master (no external fixture) ----
if __name__ == "__main__":
    rng = np.random.default_rng(7)
    m = np.clip(rng.normal(128, 40, (270, 480)), 0, 255)
    for (S, A, TX, TY) in [(1.15, 7.0, 10, -6), (0.85, -4.0, -8, 5), (1.30, 0.0, 0, 0)]:
        dist = warp(m, S, A, TX, TY)
        aligned, est = register(dist, m)
        before = np.abs(dist - m).mean()
        after = np.abs(aligned - m).mean()
        print(f"truth s={S:.2f} a={A:+.1f}  ->  est s={est['scale']:.3f} a={est['angle']:+.1f} "
              f"t=({est['tx']:+d},{est['ty']:+d})   residual {before:.1f} -> {after:.1f}  "
              f"{'OK' if after < before * 0.6 else 'WEAK'}")
