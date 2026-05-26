export function keysymForBrowserKey(key) {
  const named = {
    Enter: 65293,
    Backspace: 65288,
    Delete: 65535,
    Escape: 65307,
    Tab: 65289,
    ArrowLeft: 65361,
    ArrowUp: 65362,
    ArrowRight: 65363,
    ArrowDown: 65364,
    Home: 65360,
    End: 65367,
    PageUp: 65365,
    PageDown: 65366,
    " ": 32,
  };
  if (Object.hasOwn(named, key)) {
    return named[key];
  }
  if (typeof key === "string" && [...key].length === 1) {
    return key.codePointAt(0);
  }
  return null;
}

export function selkiesKeypressMessagesForBrowserKey(key) {
  if (typeof key === "string" && [...key].length === 1) {
    return [`co,end,${key}`];
  }
  const keysym = keysymForBrowserKey(key);
  return keysym == null ? [] : [`kd,${keysym}`, `ku,${keysym}`];
}

export function base64Utf8(value) {
  const bytes = new TextEncoder().encode(String(value || ""));
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}

export function utf8FromBase64(value) {
  const binary = atob(String(value || ""));
  const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

export function selkiesMessagesForInput(event, currentView = null) {
  if (!event || typeof event !== "object") {
    return [];
  }
  if (event.type === "clipboard_write") {
    const text = String(event.text || "");
    return text ? [`cw,${base64Utf8(text)}`] : [];
  }
  if (event.type === "clipboard_read") {
    return ["cr"];
  }
  if (event.type === "key_combo") {
    const keysyms = Array.isArray(event.keysyms)
      ? event.keysyms.filter((value) => Number.isInteger(value))
      : [];
    return keysyms.length > 0
      ? [...keysyms.map((keysym) => `kd,${keysym}`), ...[...keysyms].reverse().map((keysym) => `ku,${keysym}`)]
      : [];
  }
  if (event.type === "click") {
    const x = Math.round(Number(event.x || 0));
    const y = Math.round(Number(event.y || 0));
    return [`m,${x},${y},0,0`, `m,${x},${y},1,0`, `m,${x},${y},0,0`];
  }
  if (event.type === "wheel") {
    const magnitude = Math.max(1, Math.min(12, Math.ceil(Math.abs(Number(event.delta_y || 0)) / 80)));
    const x = Math.round(Number(event.x || (currentView?.width || 1280) / 2));
    const y = Math.round(Number(event.y || (currentView?.height || 720) / 2));
    if (Number(event.delta_y || 0) < 0) {
      return [`m,${x},${y},16,${magnitude}`, `m,${x},${y},0,0`];
    }
    if (Number(event.delta_y || 0) > 0) {
      return [`m,${x},${y},8,${magnitude}`, `m,${x},${y},0,0`];
    }
    return [];
  }
  if (event.type === "key") {
    return selkiesKeypressMessagesForBrowserKey(event.key);
  }
  if (event.type === "text") {
    const text = String(event.text || "");
    return text ? [`co,end,${text}`] : [];
  }
  return [];
}
