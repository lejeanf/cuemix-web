// Microphone capture for the room test (browser only; the maths is in roomEq.ts).
import { averagePower, dbToPower } from './roomEq';

export const FFT_SIZE = 32_768;

export type MicSupport = 'ok' | 'insecure' | 'unsupported';

/** Whether the page may use the microphone (needs a secure context). */
export function micSupport(): MicSupport {
  if (typeof window === 'undefined') return 'unsupported';
  if (!window.isSecureContext) return 'insecure';
  if (!navigator.mediaDevices || typeof navigator.mediaDevices.getUserMedia !== 'function') return 'unsupported';
  return 'ok';
}

/** Microphone through an AnalyserNode; `sample()` stores one power spectrum, `take()` averages and clears them. */
export class MicCapture {
  readonly sampleRate: number;
  private frames: Float32Array[] = [];
  private readonly buf: Float32Array<ArrayBuffer>;
  private readonly wave: Float32Array<ArrayBuffer>;
  private closed = false;

  private constructor(
    private readonly stream: MediaStream,
    private readonly ctx: AudioContext,
    private readonly analyser: AnalyserNode,
  ) {
    this.sampleRate = ctx.sampleRate;
    this.buf = new Float32Array(analyser.frequencyBinCount);
    this.wave = new Float32Array(analyser.fftSize);
  }

  /** Asks for the microphone with every processing stage off (they would level the noise out). */
  static async open(): Promise<MicCapture> {
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: false, noiseSuppression: false, autoGainControl: false },
    });
    let ctx: AudioContext | null = null;
    try {
      ctx = new AudioContext();
      if (ctx.state === 'suspended') await ctx.resume();
      const src = ctx.createMediaStreamSource(stream);
      const analyser = ctx.createAnalyser();
      analyser.fftSize = FFT_SIZE;
      analyser.smoothingTimeConstant = 0;
      src.connect(analyser);
      return new MicCapture(stream, ctx, analyser);
    } catch (e) {
      stream.getTracks().forEach((t) => t.stop());
      void ctx?.close().catch(() => {});
      throw e;
    }
  }

  /** Stores the current power spectrum (linear power per bin). */
  sample(): void {
    if (this.closed) return;
    this.analyser.getFloatFrequencyData(this.buf);
    // dB of a full-scale sine-ish scale; only ratios matter here.
    this.frames.push(Float32Array.from(this.buf, dbToPower));
  }

  /** RMS level of the latest window, dBFS (for the level meter). */
  level(): number {
    if (this.closed) return -Infinity;
    this.analyser.getFloatTimeDomainData(this.wave);
    let s = 0;
    for (const v of this.wave) s += v * v;
    return 10 * Math.log10(s / this.wave.length + 1e-12);
  }

  /** Mean of the stored spectra; clears them. */
  take(): Float64Array {
    const out = averagePower(this.frames);
    this.frames = [];
    return out;
  }

  /** Stops every track and closes the audio context. Safe to call twice. */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.stream.getTracks().forEach((t) => t.stop());
    void this.ctx.close().catch(() => {});
  }
}
