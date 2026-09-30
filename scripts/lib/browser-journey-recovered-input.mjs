export function recoveredInputLiftPx(rect) {
  if (!rect || !Number.isFinite(rect.y) || !Number.isFinite(rect.height)) return 0;
  if (rect.y + rect.height * 0.5 >= 0) return 0;
  return Math.min(1920, Math.max(640, Math.ceil(-(rect.y + rect.height * 0.5) / 640) * 640));
}

export function recoveredInputClickRect(lastRect, restRect) {
  const lift = recoveredInputLiftPx(lastRect);
  if (!lift) return { rect: lastRect, lift: 0 };
  return {
    lift,
    rect: restRect ? { ...restRect } : { ...lastRect, y: Math.max(0, lastRect.y + lift) },
  };
}
