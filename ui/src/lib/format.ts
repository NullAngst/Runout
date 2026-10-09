// Small formatting helpers. Labels always carry their unit.

export function time(s: number, ms = false): string {
  if (!isFinite(s) || s < 0) s = 0;
  const m = Math.floor(s / 60);
  const r = s - m * 60;
  return ms ? `${m}:${r.toFixed(3).padStart(6, '0')}` : `${m}:${Math.floor(r).toString().padStart(2, '0')}`;
}

export function db(v: number | null | undefined, digits = 1, unit = 'dB'): string {
  if (v == null || !isFinite(v) || v <= -69.9) return `-inf ${unit}`;
  return `${v.toFixed(digits)} ${unit}`;
}

export function signed(v: number, digits = 1, unit = 'dB'): string {
  return `${v >= 0 ? '+' : ''}${v.toFixed(digits)} ${unit}`;
}

export function dur(s: number): string {
  return time(s);
}

/** Frequency at a spectrogram row (log scale from 20 Hz), same formula as the engine. */
export function rowFreq(yFrac: number, nyq: number): number {
  const fmin = 20;
  return fmin * Math.pow(nyq / fmin, 1 - yFrac);
}

export function hz(f: number): string {
  return f >= 1000 ? `${(f / 1000).toFixed(f >= 10000 ? 0 : 1)} kHz` : `${Math.round(f)} Hz`;
}
