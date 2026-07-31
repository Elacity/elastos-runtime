(() => {
  if (window.__elastosGbaProductProbeInstalled) return;
  window.__elastosGbaProductProbeInstalled = true;

  window.__elastosGbaTrustedInput = {
    keydown_trusted: false,
    keyup_trusted: false,
    pressed: false,
    released: false,
    start_keydown_trusted: false,
    start_keyup_trusted: false,
    start_pressed: false,
    start_released: false,
  };
  const afterProductInput = (callback) => window.setTimeout(callback, 0);
  window.addEventListener("keydown", (event) => {
    if (event.code !== "KeyX") return;
    window.__elastosGbaTrustedInput.keydown_trusted = event.isTrusted;
    afterProductInput(() => {
      window.__elastosGbaTrustedInput.pressed =
        window.__elastosGbaTrustedInput.pressed ||
        document.querySelector('[data-key="a"]')?.classList.contains("pressed") === true;
    });
  }, { capture: true });
  window.addEventListener("keyup", (event) => {
    if (event.code !== "KeyX") return;
    window.__elastosGbaTrustedInput.keyup_trusted = event.isTrusted;
    afterProductInput(() => {
      window.__elastosGbaTrustedInput.released =
        window.__elastosGbaTrustedInput.released ||
        document.querySelector('[data-key="a"]')?.classList.contains("pressed") === false;
    });
  }, { capture: true });
  window.addEventListener("keydown", (event) => {
    if (event.code !== "Enter") return;
    window.__elastosGbaTrustedInput.start_keydown_trusted = event.isTrusted;
    afterProductInput(() => {
      window.__elastosGbaTrustedInput.start_pressed =
        window.__elastosGbaTrustedInput.start_pressed ||
        document.querySelector('[data-key="start"]')?.classList.contains("pressed") === true;
    });
  }, { capture: true });
  window.addEventListener("keyup", (event) => {
    if (event.code !== "Enter") return;
    window.__elastosGbaTrustedInput.start_keyup_trusted = event.isTrusted;
    afterProductInput(() => {
      window.__elastosGbaTrustedInput.start_released =
        window.__elastosGbaTrustedInput.start_released ||
        document.querySelector('[data-key="start"]')?.classList.contains("pressed") === false;
    });
  }, { capture: true });

  const render = {
    install_calls: 0,
    install_context_available: false,
    install_error: "",
    context_count: 0,
    context_type: "",
    put_image_data_calls: 0,
    draw_calls: 0,
    texture_uploads: 0,
    framebuffer_uploads: 0,
    changed_framebuffer_uploads: 0,
    framebuffer_hashes: [],
    last_framebuffer_hash: "",
    max_framebuffer_bytes: 0,
    nonzero_framebuffer_bytes: 0,
    changed_frame_writes: 0,
    unique_frame_hashes: [],
    last_frame_hash: "",
    max_image_data_bytes: 0,
    nonzero_image_data_bytes: 0,
    last_write_at: 0,
  };
  window.__elastosGbaRenderProbe = render;

  const hashFrame = (imageData) => {
    const data = imageData?.data;
    if (!data?.byteLength) return "";
    const pixels = new Uint32Array(data.buffer, data.byteOffset, data.byteLength / 4);
    let hash = 2166136261;
    for (const pixel of pixels) {
      hash = Math.imul(hash ^ pixel, 16777619);
    }
    hash = Math.imul(hash ^ Number(imageData.width || 0), 16777619);
    hash = Math.imul(hash ^ Number(imageData.height || 0), 16777619);
    return (hash >>> 0).toString(16).padStart(8, "0");
  };

  const hashBytes = (bytes) => {
    let hash = 2166136261;
    for (const byte of bytes) hash = Math.imul(hash ^ byte, 16777619);
    return (hash >>> 0).toString(16).padStart(8, "0");
  };

  const recordFramebufferUpload = (args) => {
    const pixels = [...args].reverse().find((value) => ArrayBuffer.isView(value));
    if (!pixels || pixels.byteLength < 240 * 160 * 2) return;
    const bytes = new Uint8Array(pixels.buffer, pixels.byteOffset, pixels.byteLength);
    const frameHash = hashBytes(bytes);
    render.framebuffer_uploads += 1;
    render.max_framebuffer_bytes = Math.max(render.max_framebuffer_bytes, bytes.byteLength);
    let nonzero = 0;
    for (const byte of bytes) {
      if (byte !== 0) nonzero += 1;
    }
    render.nonzero_framebuffer_bytes = Math.max(render.nonzero_framebuffer_bytes, nonzero);
    if (frameHash !== render.last_framebuffer_hash) {
      if (render.last_framebuffer_hash) render.changed_framebuffer_uploads += 1;
      render.last_framebuffer_hash = frameHash;
      if (render.framebuffer_hashes.length < 16 &&
          !render.framebuffer_hashes.includes(frameHash)) {
        render.framebuffer_hashes.push(frameHash);
      }
    }
  };

  const recordFrame = (imageData) => {
    const frameHash = hashFrame(imageData);
    const bytes = imageData?.data
      ? new Uint8Array(
          imageData.data.buffer,
          imageData.data.byteOffset,
          imageData.data.byteLength,
        )
      : new Uint8Array();
    render.put_image_data_calls += 1;
    render.max_image_data_bytes = Math.max(render.max_image_data_bytes, bytes.byteLength);
    let nonzero = 0;
    for (const byte of bytes) {
      if (byte !== 0) nonzero += 1;
    }
    render.nonzero_image_data_bytes = Math.max(render.nonzero_image_data_bytes, nonzero);
    render.last_write_at = performance.now();
    if (frameHash && frameHash !== render.last_frame_hash) {
      if (render.last_frame_hash) render.changed_frame_writes += 1;
      render.last_frame_hash = frameHash;
      if (render.unique_frame_hashes.length < 16 &&
          !render.unique_frame_hashes.includes(frameHash)) {
        render.unique_frame_hashes.push(frameHash);
      }
    }
  };

  const instrumentRenderContext = (context, type = "2d") => {
    if (!context || context.__elastosGbaRenderProbeInstalled) return context;
    Object.defineProperty(context, "__elastosGbaRenderProbeInstalled", { value: true });
    render.context_count += 1;
    render.context_type = String(context.constructor?.name || type || "unknown");
    if (typeof context.putImageData === "function") {
      const nativePutImageData = context.putImageData.bind(context);
      context.putImageData = (imageData, ...putArgs) => {
        const result = nativePutImageData(imageData, ...putArgs);
        recordFrame(imageData);
        return result;
      };
    }
    for (const method of ["drawArrays", "drawElements"]) {
      if (typeof context[method] !== "function") continue;
      const nativeDraw = context[method].bind(context);
      context[method] = (...args) => {
        const result = nativeDraw(...args);
        render.draw_calls += 1;
        render.last_write_at = performance.now();
        return result;
      };
    }
    for (const method of ["texImage2D", "texSubImage2D"]) {
      if (typeof context[method] !== "function") continue;
      const nativeUpload = context[method].bind(context);
      context[method] = (...args) => {
        const result = nativeUpload(...args);
        render.texture_uploads += 1;
        render.last_write_at = performance.now();
        recordFramebufferUpload(args);
        return result;
      };
    }
    return context;
  };

  const canvasPrototype = window.HTMLCanvasElement?.prototype;
  const nativeGetContext = canvasPrototype?.getContext;
  if (nativeGetContext) {
    canvasPrototype.getContext = function probedGetContext(...args) {
      const context = nativeGetContext.apply(this, args);
      return this.id === "canvas"
        ? instrumentRenderContext(context, args[0])
        : context;
    };
  }
  window.__elastosInstallGbaCanvasProbe = (canvas) => {
    render.install_calls += 1;
    try {
      const context = canvas?.getContext("2d") ||
        canvas?.getContext("webgl2") ||
        canvas?.getContext("webgl") ||
        canvas?.getContext("experimental-webgl");
      render.install_context_available = Boolean(context);
      return instrumentRenderContext(context, "2d");
    } catch (error) {
      render.install_error = String(error?.message || error);
      return null;
    }
  };

  const audio = {
    contexts: [],
    script_processor_callbacks: 0,
    rendered_buffers: 0,
    samples_examined: 0,
    nonzero_samples: 0,
    max_abs_sample: 0,
  };
  window.__elastosGbaAudioProbe = audio;

  const recordAudioBuffer = (buffer) => {
    if (!buffer || typeof buffer.getChannelData !== "function") return;
    audio.rendered_buffers += 1;
    for (let channel = 0; channel < buffer.numberOfChannels; channel += 1) {
      const samples = buffer.getChannelData(channel);
      audio.samples_examined += samples.length;
      for (const sample of samples) {
        const absolute = Math.abs(sample);
        if (absolute > audio.max_abs_sample) audio.max_abs_sample = absolute;
        if (absolute > 1e-8) audio.nonzero_samples += 1;
      }
    }
  };

  const processorPrototype = window.ScriptProcessorNode?.prototype;
  const audioProcessDescriptor = processorPrototype &&
    Object.getOwnPropertyDescriptor(processorPrototype, "onaudioprocess");
  if (audioProcessDescriptor?.set) {
    Object.defineProperty(processorPrototype, "onaudioprocess", {
      configurable: audioProcessDescriptor.configurable,
      enumerable: audioProcessDescriptor.enumerable,
      get: audioProcessDescriptor.get,
      set(callback) {
        if (typeof callback !== "function") {
          audioProcessDescriptor.set.call(this, callback);
          return;
        }
        audioProcessDescriptor.set.call(this, function probedAudioProcess(event) {
          try {
            return callback.call(this, event);
          } finally {
            audio.script_processor_callbacks += 1;
            recordAudioBuffer(event.outputBuffer);
          }
        });
      },
    });
  }

  const NativeAudioContext = window.AudioContext || window.webkitAudioContext;
  if (!NativeAudioContext) return;
  class ProbedAudioContext extends NativeAudioContext {
    constructor(...args) {
      super(...args);
      audio.contexts.push(this);
    }
  }
  window.AudioContext = ProbedAudioContext;
  if (window.webkitAudioContext) window.webkitAudioContext = ProbedAudioContext;
})();
