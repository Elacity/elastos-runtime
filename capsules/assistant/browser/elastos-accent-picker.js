/* GENERATED from capsules/_shared/elastos-accent-picker.js — do not edit. Run `just vendor-ui`. */
/* ElastOS UI — lightweight in-app accent color picker.
 *
 * Replaces the OS <input type="color"> dialog (sharp system chrome) with a
 * rounded ElastOS popover: SV pad + hue slider. Hex stays in the host UI.
 * Vendored next to elastos-theme.js by `just vendor-ui`.
 */
(function () {
  const DEFAULT_HEX = "#4f7fff";

  function clamp(value, min, max) {
    return Math.min(max, Math.max(min, value));
  }

  function normalizeHex(value) {
    if (typeof window.elastosTheme?.normalizeHex === "function") {
      return window.elastosTheme.normalizeHex(value);
    }
    if (typeof value !== "string") {
      return "";
    }
    let hex = value.trim();
    if (/^#[0-9A-Fa-f]{3}$/.test(hex)) {
      hex = `#${hex[1]}${hex[1]}${hex[2]}${hex[2]}${hex[3]}${hex[3]}`;
    }
    return /^#[0-9A-Fa-f]{6}$/.test(hex) ? hex.toLowerCase() : "";
  }

  function hexToHsv(hex) {
    const normalized = normalizeHex(hex) || DEFAULT_HEX;
    const r = Number.parseInt(normalized.slice(1, 3), 16) / 255;
    const g = Number.parseInt(normalized.slice(3, 5), 16) / 255;
    const b = Number.parseInt(normalized.slice(5, 7), 16) / 255;
    const max = Math.max(r, g, b);
    const min = Math.min(r, g, b);
    const delta = max - min;
    let h = 0;
    if (delta !== 0) {
      if (max === r) {
        h = ((g - b) / delta) % 6;
      } else if (max === g) {
        h = (b - r) / delta + 2;
      } else {
        h = (r - g) / delta + 4;
      }
      h *= 60;
      if (h < 0) {
        h += 360;
      }
    }
    const s = max === 0 ? 0 : delta / max;
    return { h, s, v: max };
  }

  function hsvToHex(h, s, v) {
    const hue = ((h % 360) + 360) % 360;
    const c = v * s;
    const x = c * (1 - Math.abs(((hue / 60) % 2) - 1));
    const m = v - c;
    let r = 0;
    let g = 0;
    let b = 0;
    if (hue < 60) {
      r = c;
      g = x;
    } else if (hue < 120) {
      r = x;
      g = c;
    } else if (hue < 180) {
      g = c;
      b = x;
    } else if (hue < 240) {
      g = x;
      b = c;
    } else if (hue < 300) {
      r = x;
      b = c;
    } else {
      r = c;
      b = x;
    }
    const toByte = (channel) => Math.round((channel + m) * 255)
      .toString(16)
      .padStart(2, "0");
    return `#${toByte(r)}${toByte(g)}${toByte(b)}`;
  }

  function mount(root, options = {}) {
    if (!root || root.dataset.elAccentPickerMounted === "1") {
      return null;
    }
    const onChange = typeof options.onChange === "function" ? options.onChange : () => {};
    const getHex = typeof options.getHex === "function"
      ? options.getHex
      : () => DEFAULT_HEX;

    root.dataset.elAccentPickerMounted = "1";
    root.classList.add("el-accent-picker");
    root.innerHTML = `
      <button type="button" class="el-accent-picker-swatch" aria-label="Open color picker" aria-expanded="false" aria-haspopup="dialog"></button>
      <div class="el-accent-picker-pop" role="dialog" aria-label="Custom accent color" hidden>
        <div class="el-accent-picker-sv" tabindex="0" aria-label="Saturation and brightness">
          <span class="el-accent-picker-sv-thumb" aria-hidden="true"></span>
        </div>
        <input class="el-accent-picker-hue" type="range" min="0" max="360" step="1" aria-label="Hue" />
      </div>
    `;

    const swatch = root.querySelector(".el-accent-picker-swatch");
    const pop = root.querySelector(".el-accent-picker-pop");
    const sv = root.querySelector(".el-accent-picker-sv");
    const thumb = root.querySelector(".el-accent-picker-sv-thumb");
    const hueInput = root.querySelector(".el-accent-picker-hue");
    let hsv = hexToHsv(getHex());
    let dragging = false;

    function paint() {
      const hex = hsvToHex(hsv.h, hsv.s, hsv.v);
      const hueColor = hsvToHex(hsv.h, 1, 1);
      swatch.style.background = hex;
      sv.style.background = `
        linear-gradient(to top, #000, transparent),
        linear-gradient(to right, #fff, ${hueColor})
      `;
      thumb.style.left = `${hsv.s * 100}%`;
      thumb.style.top = `${(1 - hsv.v) * 100}%`;
      hueInput.value = String(Math.round(hsv.h));
      hueInput.style.setProperty("--el-accent-picker-hue", hueColor);
      return hex;
    }

    function emit(hex) {
      onChange(hex);
    }

    function setFromPoint(clientX, clientY) {
      const rect = sv.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) {
        return;
      }
      hsv.s = clamp((clientX - rect.left) / rect.width, 0, 1);
      hsv.v = clamp(1 - ((clientY - rect.top) / rect.height), 0, 1);
      emit(paint());
    }

    function openPop() {
      pop.hidden = false;
      swatch.setAttribute("aria-expanded", "true");
      paint();
    }

    function closePop() {
      pop.hidden = true;
      swatch.setAttribute("aria-expanded", "false");
    }

    function togglePop() {
      if (pop.hidden) {
        openPop();
      } else {
        closePop();
      }
    }

    swatch.addEventListener("click", (event) => {
      event.preventDefault();
      event.stopPropagation();
      togglePop();
    });

    hueInput.addEventListener("input", () => {
      hsv.h = Number(hueInput.value) || 0;
      emit(paint());
    });

    sv.addEventListener("pointerdown", (event) => {
      dragging = true;
      sv.setPointerCapture?.(event.pointerId);
      setFromPoint(event.clientX, event.clientY);
    });
    sv.addEventListener("pointermove", (event) => {
      if (!dragging) {
        return;
      }
      setFromPoint(event.clientX, event.clientY);
    });
    sv.addEventListener("pointerup", () => {
      dragging = false;
    });
    sv.addEventListener("pointercancel", () => {
      dragging = false;
    });

    const onDocPointer = (event) => {
      if (pop.hidden) {
        return;
      }
      if (root.contains(event.target)) {
        return;
      }
      closePop();
    };
    document.addEventListener("pointerdown", onDocPointer, true);

    paint();

    return {
      setHex(hex) {
        const normalized = normalizeHex(hex);
        if (!normalized) {
          return;
        }
        hsv = hexToHsv(normalized);
        paint();
      },
      open: openPop,
      close: closePop,
      destroy() {
        document.removeEventListener("pointerdown", onDocPointer, true);
      },
    };
  }

  window.elastosAccentPicker = {
    mount,
    hexToHsv,
    hsvToHex,
    normalizeHex,
  };
})();
