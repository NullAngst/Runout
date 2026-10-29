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

/** "3:41", "1:02:03" or "221" (seconds) to seconds; null if it does not parse. */
export function parseTime(s: string): number | null {
  const parts = s.trim().split(':').map((x) => x.trim());
  if (!parts.length || parts.some((x) => x === '' || !isFinite(Number(x)))) return null;
  return parts.reduce((acc, x) => acc * 60 + Number(x), 0);
}

/** A speed with its unit: "33 1/3 rpm", "4.76 cm/s", "7.5 ips". */
export function speed(v: number, unit: string): string {
  if (unit === 'rpm') return `${Math.abs(v - 33.333) < 0.01 ? '33 1/3' : nice(v)} rpm`;
  if (unit === 'cms') return `${nice(v)} cm/s`;
  if (unit === 'ips') return `${nice(v)} ips`;
  return `${nice(v)}`;
}

/** A number stored as f32 in Rust, without the float noise: 4.76, not 4.760000228881836. */
export function nice(v: number): number {
  return Number(v.toPrecision(6));
}
