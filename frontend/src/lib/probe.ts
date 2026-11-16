/** Rolling statistics over the last raw meter frames, for the meter probe. */
export interface ProbeStats {
  mean: number[];
  min: number[];
  max: number[];
}

export class ProbeWindow {
  private frames: number[][] = [];
  constructor(private size = 45) {}

  push(frame: readonly number[]): void {
    this.frames.push([...frame]);
    if (this.frames.length > this.size) this.frames.shift();
  }

  clear(): void {
    this.frames = [];
  }

  get count(): number {
    return this.frames.length;
  }

  stats(): ProbeStats {
    const n = this.frames.reduce((m, f) => Math.max(m, f.length), 0);
    const mean = new Array<number>(n).fill(0);
    const min = new Array<number>(n).fill(Infinity);
    const max = new Array<number>(n).fill(-Infinity);
    for (let i = 0; i < n; i++) {
      let sum = 0;
      let k = 0;
      for (const f of this.frames) {
        const v = f[i];
        if (v === undefined) continue;
        sum += v;
        k++;
        if (v < min[i]) min[i] = v;
        if (v > max[i]) max[i] = v;
      }
      mean[i] = k ? sum / k : 0;
      if (!k) min[i] = max[i] = 0;
    }
    return { mean, min, max };
  }
}

/** Indices whose mean moved by more than `threshold` from the baseline. */
export function changedFrom(baseline: readonly number[], mean: readonly number[], threshold = 6): number[] {
  const out: number[] = [];
  mean.forEach((m, i) => {
    const b = baseline[i];
    if (b !== undefined && Math.abs(m - b) > threshold) out.push(i);
  });
  return out;
}
