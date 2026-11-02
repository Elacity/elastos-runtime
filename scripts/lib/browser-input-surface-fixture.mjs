import { bindBrowserInputSurface } from "../../capsules/browser/browser/browser-input-surface.js";

class Surface {
  handlers = new Map();
  addEventListener(name, handler) {
    const handlers = this.handlers.get(name) || []; handlers.push(handler); this.handlers.set(name, handlers);
  }
  emit(name, fields = {}) {
    const event = { target: this, preventDefault() {}, stopPropagation() {}, ...fields };
    for (const handler of this.handlers.get(name) || []) handler(event);
  }
  focus() {} setPointerCapture() {} releasePointerCapture() {}
}

export function inputSurfaceFixture(width = 1000, height = 700, rect = { left: 0, top: 0, width, height }) {
  globalThis.window = Object.assign(new Surface(), { setTimeout, clearTimeout });
  globalThis.document = new Surface();
  const panel = new Surface(), video = Object.assign(new Surface(), {
    hidden: false, videoWidth: width, videoHeight: height, getBoundingClientRect: () => rect,
    ownerDocument: { defaultView: { getComputedStyle: () => ({ objectFit: "contain" }) } },
  });
  const sent = [], pasted = [], copied = [], audio = [];
  let blocked = false;
  bindBrowserInputSurface({ getCurrentPage: () => ({ page_id: "page" }), getInputOwner: () => "page",
    sameInputOwner: (a, b) => a === b, getCurrentView: () => ({ width: 1920, height: 1080 }),
    inputBlocked: () => blocked, remoteVideo: video, renderPanel: panel, keyboardCapture: null,
    sendBrowserInput: async event => { sent.push(event); }, releaseDragButton: async event => { sent.push(event); },
    copyRemoteClipboardToHost: async () => { copied.push(true); },
    readHostClipboardText: async () => "host text", pasteHostClipboardIntoRemote: async text => { pasted.push(text); },
    showStatus: () => {}, friendlyOpenError: String, streamsPointerDrag: () => true,
    unlockRemoteAudioFromGesture: () => { audio.push(true); },
  });
  return { panel, video, sent, pasted, copied, audio, block: value => { blocked = value; } };
}
