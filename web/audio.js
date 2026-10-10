// Sound output: an AudioContext with the gase worklet (audio-worklet.js
// explains the two ways samples reach it).

const MAX_QUEUED_SECONDS = 0.25;

export class AudioOut {
  constructor() {
    const Context = window.AudioContext || window.webkitAudioContext;
    // A context created outside a click starts "suspended": browsers only
    // let a page make sound after the user interacted with it. Creating it
    // now still tells us the device's sample rate, which the emulator
    // resamples to.
    this.ctx = new Context({ latencyHint: 'interactive' });
    this.rate = this.ctx.sampleRate;
    this.node = null;
    this.shared = self.crossOriginIsolated === true && typeof SharedArrayBuffer === 'function';
    this.written = 0; // frames sent
    this.played = 0; // frames the worklet reported as played (message mode)
    this.reportTime = 0;
    // On iPhones, Web Audio is silenced by the ring/silent switch unless
    // the page says it is a media player.
    if (navigator.audioSession) navigator.audioSession.type = 'playback';
  }

  get mode() {
    return this.shared ? 'SharedArrayBuffer ring' : 'postMessage chunks';
  }

  async start(url) {
    await this.ctx.audioWorklet.addModule(url);
    const capacity = this.rate; // one second of stereo frames
    let ring = null;
    if (this.shared) {
      ring = new SharedArrayBuffer(8 + capacity * 4);
      this.index = new Int32Array(ring, 0, 2);
      this.ring = new Int16Array(ring, 8, capacity * 2);
      this.capacity = capacity;
    }
    this.node = new AudioWorkletNode(this.ctx, 'gase-audio', {
      numberOfInputs: 0,
      outputChannelCount: [2],
      processorOptions: { ring, capacity, maxQueued: Math.round(this.rate * MAX_QUEUED_SECONDS) },
    });
    this.node.port.onmessage = (e) => {
      this.played = e.data.played;
      this.reportTime = e.data.time;
    };
    this.node.connect(this.ctx.destination);
  }

  /** Call from a click or key handler: the browser may now allow sound. */
  resume() {
    if (this.ctx.state !== 'running') this.ctx.resume().catch(() => {});
  }

  get running() {
    return this.node !== null && this.ctx.state === 'running';
  }

  /** Stereo frames waiting to be played, or -1 while there is no sound. */
  queued() {
    if (!this.running) return -1;
    if (this.shared) {
      return Math.max(0, (Atomics.load(this.index, 0) - Atomics.load(this.index, 1)) | 0);
    }
    // The last report, moved forward by the audio clock since.
    const since = Math.max(0, this.ctx.currentTime - this.reportTime);
    const played = this.played + since * this.rate;
    return Math.max(0, Math.round(this.written - played));
  }

  /** Queue interleaved stereo samples (a view into wasm memory). */
  push(samples) {
    if (!this.running) return;
    const frames = samples.length >> 1;
    if (this.shared) {
      const written = Atomics.load(this.index, 0);
      const read = Atomics.load(this.index, 1);
      if (((written - read) | 0) + frames > this.capacity) return; // full: drop
      const at = (written >>> 0) % this.capacity;
      const first = Math.min(frames, this.capacity - at);
      this.ring.set(samples.subarray(0, first * 2), at * 2);
      this.ring.set(samples.subarray(first * 2), 0);
      Atomics.store(this.index, 0, (written + frames) | 0);
    } else {
      // The view points into WebAssembly memory, which cannot be handed to
      // another thread: copy it (slice) and transfer the copy.
      const copy = samples.slice();
      this.node.port.postMessage(copy, [copy.buffer]);
      this.written += frames;
    }
  }
}
