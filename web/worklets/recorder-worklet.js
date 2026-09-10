// recorder-worklet.js — AudioWorkletProcessor that batches raw microphone
// Float32 frames (context sample rate, mono-mixed) and posts them to the
// main thread every BATCH_FRAMES samples (~42 ms @48 kHz).

const BATCH_FRAMES = 2048;

class RecorderProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this._buf = new Float32Array(BATCH_FRAMES);
    this._fill = 0;
  }

  process(inputs) {
    const input = inputs[0];
    if (!input || input.length === 0) return true;

    // Mono-mix every channel.
    const ch0 = input[0];
    const chCount = input.length;
    for (let i = 0; i < ch0.length; i++) {
      let s = ch0[i];
      if (chCount > 1) {
        for (let c = 1; c < chCount; c++) s += input[c][i];
        s /= chCount;
      }
      this._buf[this._fill++] = s;
      if (this._fill === BATCH_FRAMES) {
        this.port.postMessage(this._buf.slice(0));
        this._fill = 0;
      }
    }
    return true;
  }
}

registerProcessor("recorder-processor", RecorderProcessor);
