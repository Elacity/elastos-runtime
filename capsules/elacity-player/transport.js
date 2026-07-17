// Custom transport for the protected player. Pure presentation: every control
// drives the SAME <video> element the stream module feeds — no new routes, no
// new authority, nothing the native controls could not already do.

const video = document.getElementById("video");
const frame = document.querySelector(".video-frame");
const bar = document.getElementById("transport");
const playButton = document.getElementById("transport-play");
const muteButton = document.getElementById("transport-mute");
const timeLabel = document.getElementById("transport-time");
const seek = document.getElementById("transport-seek");
const volume = document.getElementById("transport-volume");
const fullscreenButton = document.getElementById("transport-fullscreen");

const HIDE_AFTER_MS = 2600;
let hideTimer = 0;
let scrubbing = false;

function formatTime(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) {
    return "0:00";
  }
  const whole = Math.floor(seconds);
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  return `${h > 0 ? h + ":" : ""}${mm}:${String(s).padStart(2, "0")}`;
}

function syncPlayState() {
  const playing = !video.paused && !video.ended;
  playButton.dataset.state = playing ? "playing" : "paused";
  playButton.setAttribute("aria-label", playing ? "Pause" : "Play");
  syncVisibility();
}

function syncMuteState() {
  muteButton.dataset.state = video.muted || video.volume === 0 ? "muted" : "on";
  muteButton.setAttribute("aria-label", video.muted ? "Unmute" : "Mute");
  volume.value = video.muted ? "0" : String(Math.round(video.volume * 100));
}

function syncTime() {
  if (!scrubbing) {
    const duration = video.duration;
    seek.max = Number.isFinite(duration) && duration > 0 ? String(duration) : "0";
    seek.value = String(video.currentTime || 0);
  }
  timeLabel.textContent = `${formatTime(video.currentTime)} / ${formatTime(video.duration)}`;
}

function togglePlay() {
  if (video.paused || video.ended) {
    video.play().catch(() => {});
  } else {
    video.pause();
  }
}

function toggleFullscreen() {
  if (document.fullscreenElement) {
    document.exitFullscreen?.().catch(() => {});
  } else {
    frame.requestFullscreen?.().catch(() => {});
  }
}

/* Idle auto-hide while playing; always visible when paused or focused. */
function showTransport() {
  bar.dataset.visible = "true";
  frame.classList.remove("cursor-idle");
  window.clearTimeout(hideTimer);
  hideTimer = window.setTimeout(() => {
    const playing = !video.paused && !video.ended;
    if (playing && !bar.contains(document.activeElement) && !scrubbing) {
      bar.dataset.visible = "false";
      frame.classList.add("cursor-idle");
    }
  }, HIDE_AFTER_MS);
}

function syncVisibility() {
  showTransport();
}

playButton.addEventListener("click", togglePlay);
fullscreenButton.addEventListener("click", toggleFullscreen);
video.addEventListener("click", togglePlay);
video.addEventListener("dblclick", toggleFullscreen);

muteButton.addEventListener("click", () => {
  video.muted = !video.muted;
});

volume.addEventListener("input", () => {
  video.volume = Number(volume.value) / 100;
  video.muted = video.volume === 0;
});

seek.addEventListener("input", () => {
  scrubbing = true;
  timeLabel.textContent = `${formatTime(Number(seek.value))} / ${formatTime(video.duration)}`;
});
seek.addEventListener("change", () => {
  scrubbing = false;
  const target = Number(seek.value);
  if (Number.isFinite(target)) {
    video.currentTime = target;
  }
});

video.addEventListener("play", syncPlayState);
video.addEventListener("pause", syncPlayState);
video.addEventListener("ended", syncPlayState);
video.addEventListener("timeupdate", syncTime);
video.addEventListener("durationchange", syncTime);
video.addEventListener("volumechange", syncMuteState);

frame.addEventListener("pointermove", showTransport);
bar.addEventListener("focusin", showTransport);

/* Desktop-player keys: Space toggles, arrows seek ±5s, M mutes, F fullscreen.
   Skipped while a transport control has focus so ranges keep native keys. */
document.addEventListener("keydown", (event) => {
  const tag = document.activeElement?.tagName || "";
  if (tag === "INPUT" || tag === "BUTTON") {
    return;
  }
  if (event.key === " " || event.key === "k") {
    event.preventDefault();
    togglePlay();
  } else if (event.key === "ArrowRight") {
    event.preventDefault();
    video.currentTime = Math.min(video.duration || Infinity, video.currentTime + 5);
  } else if (event.key === "ArrowLeft") {
    event.preventDefault();
    video.currentTime = Math.max(0, video.currentTime - 5);
  } else if (event.key === "m") {
    video.muted = !video.muted;
  } else if (event.key === "f") {
    toggleFullscreen();
  }
  showTransport();
});

syncPlayState();
syncMuteState();
syncTime();
