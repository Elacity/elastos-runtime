// Runs in each newly created viewer document before its video is attached.
export function browserJourneyFirstFrameProbe() {
  if (!location.pathname.startsWith("/apps/browser/")) return;
  const attach = () => {
    const video = document.querySelector("#browser-remote-display");
    if (!video || typeof video.requestVideoFrameCallback !== "function") return;
    observer.disconnect();
    video.requestVideoFrameCallback((_now, metadata) => {
      window.__elastosBrowserFirstFrame = {
        observed_at: performance.timeOrigin + performance.now(),
        presented_frames: metadata.presentedFrames,
        width: metadata.width, height: metadata.height,
      };
    });
  };
  const observer = new MutationObserver(attach);
  observer.observe(document, { childList: true, subtree: true });
  attach();
}
