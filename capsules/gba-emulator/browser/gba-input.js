export const BUTTON_BITS = Object.freeze({
  a: 1 << 0,
  b: 1 << 1,
  select: 1 << 2,
  start: 1 << 3,
  right: 1 << 4,
  left: 1 << 5,
  up: 1 << 6,
  down: 1 << 7,
  r: 1 << 8,
  l: 1 << 9,
});

export function gamepadMask(pad) {
  if (!pad) return 0;
  const axisX = pad.axes?.[0] || 0;
  const axisY = pad.axes?.[1] || 0;
  const pressed = {
    left: axisX < -0.4 || Boolean(pad.buttons?.[14]?.pressed),
    right: axisX > 0.4 || Boolean(pad.buttons?.[15]?.pressed),
    up: axisY < -0.4 || Boolean(pad.buttons?.[12]?.pressed),
    down: axisY > 0.4 || Boolean(pad.buttons?.[13]?.pressed),
    a: Boolean(pad.buttons?.[0]?.pressed),
    b: Boolean(pad.buttons?.[1]?.pressed),
    select: Boolean(pad.buttons?.[8]?.pressed),
    start: Boolean(pad.buttons?.[9]?.pressed),
    l: Boolean(pad.buttons?.[4]?.pressed),
    r: Boolean(pad.buttons?.[5]?.pressed),
  };
  return Object.entries(pressed).reduce(
    (mask, [button, active]) => active ? mask | BUTTON_BITS[button] : mask,
    0,
  );
}
