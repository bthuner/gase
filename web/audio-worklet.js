// The audio worklet: plays the samples the emulator produces.
//
// An AudioWorkletProcessor runs on the browser's real-time audio thread.
// Every 128 sample frames (2.7 ms at 48 kHz) the browser calls
// `process()` and expects the next 128 frames of output, *now*: it must
// never wait for the main thread. So the emulator (on the main thread)
// and this processor share a queue: the main thread appends a frame's
// worth of samples (~800 stereo frames) about 60 times a second, and the
// processor takes 128 at a time. When the queue runs dry the processor
// plays silence (an audible click, which is why the main thread keeps
// ~50 ms queued, see gase.js and the app's `Pacing::Audio`).
//
// There are two ways to share that queue, and this file implements both:
//
// 1. A ring buffer in a SharedArrayBuffer: both threads see the same
//    memory; the writer advances a write index, the reader a read index,
//    with Atomics so each sees the other's updates in order. No messages,
//    no copies beyond the ring itself, and the main thread knows the
//    exact queue level at any moment. But browsers only allow
//    SharedArrayBuffer on "cross-origin isolated" pages, which needs two
//    HTTP headers (Cross-Origin-Opener-Policy: same-origin and
//    Cross-Origin-Embedder-Policy: require-corp). GitHub Pages and many
//    simple hosts cannot send them.
//
// 2. Messages: the main thread posts each frame's samples (transferring
//    the buffer, so nothing is copied again), the processor keeps a list
//    of chunks and posts back how many frames it has played every few
//    quanta. The main thread estimates the queue level from that report
//    plus the time elapsed since. A few milliseconds less precise, works
//    everywhere. The pacing logic tolerates that imprecision by design.

const REPORT_EVERY = 4; // quanta between "played" reports (~10 ms)

class GaseAudio extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const { ring, capacity, maxQueued } = options.processorOptions;
    this.capacity = capacity; // stereo frames
    this.maxQueued = maxQueued; // drop old audio beyond this (latency cap)
    if (ring) {
      // [0] = frames written so far, [1] = frames read so far (both wrap
      // around at 2^32; differences are still right with `| 0`).
      this.index = new Int32Array(ring, 0, 2);
      this.ring = new Int16Array(ring, 8, capacity * 2);
    } else {
      this.chunks = []; // Int16Array, interleaved stereo
      this.offset = 0; // samples already played from chunks[0]
      this.queued = 0; // frames in chunks
      this.played = 0;
      this.quanta = 0;
      this.port.onmessage = (e) => {
        this.chunks.push(e.data);
        this.queued += e.data.length >> 1;
        // If the page fell far behind (a stall, a background tab), drop the
        // oldest audio rather than play it late.
        while (this.queued > this.maxQueued && this.chunks.length > 1) {
          const old = this.chunks.shift();
          this.queued -= (old.length - this.offset) >> 1;
          this.played += (old.length - this.offset) >> 1;
          this.offset = 0;
        }
      };
    }
  }

  process(inputs, outputs) {
    const [left, right] = outputs[0];
    const n = left.length;
    let i = 0;
    if (this.ring) {
      const written = Atomics.load(this.index, 0);
      let read = Atomics.load(this.index, 1);
      const available = Math.min((written - read) | 0, n);
      for (; i < available; i++, read++) {
        const at = ((read >>> 0) % this.capacity) * 2;
        left[i] = this.ring[at] / 32768;
        right[i] = this.ring[at + 1] / 32768;
      }
      Atomics.store(this.index, 1, read);
    } else {
      while (i < n && this.chunks.length) {
        const chunk = this.chunks[0];
        while (i < n && this.offset < chunk.length) {
          left[i] = chunk[this.offset] / 32768;
          right[i] = chunk[this.offset + 1] / 32768;
          this.offset += 2;
          i++;
        }
        if (this.offset >= chunk.length) {
          this.chunks.shift();
          this.offset = 0;
        }
      }
      this.queued -= i;
      this.played += i;
      if (++this.quanta % REPORT_EVERY === 0) {
        // `currentTime` is the audio clock: the main thread extrapolates
        // from it until the next report.
        this.port.postMessage({ played: this.played, time: currentTime });
      }
    }
    // Underrun: silence for the rest of this quantum.
    left.fill(0, i);
    right.fill(0, i);
    return true;
  }
}

registerProcessor('gase-audio', GaseAudio);
