// Number, size and time formatting used across the pages. Pure functions.

export const n4 = (v: unknown) => Number(v ?? 0).toFixed(4);

export const n2 = (v: unknown) => Number(v ?? 0).toFixed(2);

/**
 * An amount of money: no decimals from 100 up, two from 1, four below (a
 * lab or a short period can be fractions of a unit), with thousands
 * separators. Four decimals on 1,234.5678 read as false precision.
 */
export function money(v: unknown): string {
  const n = Number(v ?? 0);

  return !isFinite(n) || n === 0 ? '0' : moneyAt(n, moneyDigits([n]));
}

/**
 * Decimals for a column or group of amounts, decided by its largest value
 * (≥100 → 0, ≥1 → 2, else 4). Every cell of a column uses the same count,
 * so 107 / 83.99 / 47.25 in one column does not happen.
 */
export function moneyDigits(values: unknown[]): number {
  const m = values.reduce<number>((acc, v) => {
    const n = Math.abs(Number(v ?? 0));

    return isFinite(n) ? Math.max(acc, n) : acc;
  }, 0);

  return m >= 100 ? 0 : m >= 1 ? 2 : 4;
}

/** An amount with a fixed number of decimals and thousands separators. */
export function moneyAt(v: unknown, digits: number): string {
  const n = Number(v ?? 0);

  return (isFinite(n) ? n : 0).toLocaleString('en-US', { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

/** A column's formatter: the decimals of its largest value for every cell. */
export function moneyCol(values: unknown[]): (v: unknown) => string {
  const d = moneyDigits(values);

  return (v: unknown) => moneyAt(v, d);
}

/** A ratio (0.123) as a percentage ("12.3%"). */
export const pct = (v: unknown) => `${ (Number(v ?? 0) * 100).toFixed(1) }%`;

/** A ratio, or "—" when the denominator is zero or unknown. */
export function share(part: number | null | undefined, whole: number | null | undefined): string {
  return whole && part !== null && part !== undefined ? pct(part / whole) : '—';
}

/** A percentage the backend already computed (99.6 → "99.6%"), or "—". */
export function pctValue(v: number | null | undefined): string {
  return v === null || v === undefined ? '—' : `${ v }%`;
}

export function fmtBytes(b: number | null | undefined): string {
  if (b === null || b === undefined) {
    return '—';
  }
  const u = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let v = b;
  let i = 0;

  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }

  return `${ i === 0 || v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2) } ${ u[i] }`;
}

const pad2 = (n: number) => String(n).padStart(2, '0');

// The API's times are UTC (ISO strings). The page shows them in the
// viewer's own time zone; cutting the string would put every time in
// Taiwan 8 hours off.
export function fmtLocal(iso: string | null | undefined, withDate = false): string {
  if (!iso) {
    return '—';
  }
  const t = new Date(iso);

  if (isNaN(t.getTime())) {
    return iso;
  }
  const hm = `${ pad2(t.getHours()) }:${ pad2(t.getMinutes()) }`;

  return withDate ? `${ pad2(t.getMonth() + 1) }-${ pad2(t.getDate()) } ${ hm }` : hm;
}

export function timeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || '';
  } catch {
    return '';
  }
}

/** Step "5m" → { n: 5, unit: 'm' } for translation. */
export function stepParts(step: string): { n: string; unit: 'm' | 'h' | 'd' | '' } {
  const unit = step.slice(-1);

  return ['m', 'h', 'd'].includes(unit) ? { n: step.slice(0, -1), unit: unit as 'm' | 'h' | 'd' } : { n: step, unit: '' };
}

/**
 * How many times the sample is scaled up to 30 days. Short samples make
 * the monthly estimate an extrapolation of thousands of times.
 */
export function monthFactor(minutes: number | null | undefined): number {
  return minutes ? (30 * 24 * 60) / minutes : 1;
}

function nextDay(d: string): string {
  const t = new Date(`${ d }T00:00:00Z`);

  t.setUTCDate(t.getUTCDate() + 1);

  return t.toISOString().slice(0, 10);
}

/**
 * Consecutive days folded into ranges. A list of 23 missing days is not
 * read; "09-01 ~ 09-20 (20 days)" is.
 */
export function dayRanges(days: string[]): { from: string; to: string; days: number }[] {
  const sorted = [...days].sort();
  const out: { from: string; to: string; days: number }[] = [];

  if (!sorted.length) {
    return out;
  }
  let a = sorted[0];
  let prev = sorted[0];

  for (let i = 1; i <= sorted.length; i++) {
    if (i < sorted.length && sorted[i] === nextDay(prev)) {
      prev = sorted[i];
      continue;
    }
    out.push({
      from: a.slice(5), to: prev.slice(5), days: Math.round((Date.parse(prev) - Date.parse(a)) / 86400000) + 1
    });
    a = prev = sorted[i];
  }

  return out;
}

/** The first `limit` rows, or all; a single extra row is not worth hiding. */
export function capRows<T>(rows: T[], limit: number, open: boolean): { shown: T[]; hidden: number } {
  if (open || rows.length <= limit + 1) {
    return { shown: rows, hidden: 0 };
  }

  return { shown: rows.slice(0, limit), hidden: rows.length - limit };
}
