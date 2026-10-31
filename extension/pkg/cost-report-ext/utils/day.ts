// The billing day is a UTC day. The page shows it with its local
// equivalent, because a reader in UTC+8 picking "10-30" otherwise reads
// figures from 08:00 that day to 08:00 the next as if they were their
// own calendar day.
import { fmtLocal } from './format';

const DAY_MS = 24 * 60 * 60 * 1000;

/** Start of a UTC day in ms. */
export function dayStart(day: string): number {
  return Date.parse(`${ day }T00:00:00Z`);
}

/** The UTC day's start and end in the viewer's time zone ("10-30 08:00"). */
export function dayLocalRange(day: string): { from: string; to: string } {
  const s = dayStart(day);

  return { from: fmtLocal(new Date(s).toISOString(), true), to: fmtLocal(new Date(s + DAY_MS).toISOString(), true) };
}

/** The UTC day has not ended yet (it is today, or later). */
export function isOngoingDay(day: string, now = Date.now()): boolean {
  return dayStart(day) + DAY_MS > now;
}

/** The local clock time of UTC midnight ("08:00" in Taiwan). */
export function utcMidnightLocal(now = Date.now()): string {
  const midnight = Math.floor(now / DAY_MS) * DAY_MS;

  return fmtLocal(new Date(midnight).toISOString());
}

/**
 * A date typed into the date field that is complete and plausible. While
 * typing, the field reports 0002-…, 0020-…, 0202-…; none of those may
 * trigger a query.
 */
export function completeDay(v: string): string | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(v)) {
    return null;
  }
  const y = Number(v.slice(0, 4));
  const t = dayStart(v);

  return y >= 2000 && y <= 2999 && !isNaN(t) && new Date(t).toISOString().slice(0, 10) === v ? v : null;
}

/** Why a typed date was not used; null when it can be used. */
export type DayProblem = 'invalid' | 'before' | 'after' | null;

/** Is `v` a complete date within [min, max] (both optional, YYYY-MM-DD)? */
export function checkDay(v: string, min?: string | null, max?: string | null): DayProblem {
  if (!completeDay(v)) {
    return 'invalid';
  }
  if (min && v < min) {
    return 'before';
  }
  if (max && v > max) {
    return 'after';
  }

  return null;
}

/**
 * The date field. A choice in the picker (a change without a key pressed
 * just before) applies at once; after typing, only Enter or leaving the
 * field applies it. The other parts of the date are prefilled, so each
 * typed digit is already a complete date (2026-10-3 → 2026-10-03), and no
 * pause long enough to be safe exists. A value that cannot be used is
 * reported (onProblem) instead of being dropped silently.
 */
export function dayField(opts: {
  commit: (day: string) => void;
  onProblem: (p: DayProblem) => void;
  range?: () => { min?: string | null; max?: string | null };
  keyWindowMs?: number;
  now?: () => number;
}) {
  const now = opts.now || (() => Date.now());
  const keyWindow = opts.keyWindowMs ?? 1000;
  let lastKey = -Infinity;
  let last = '';

  function apply(v: string) {
    const r = opts.range?.() || {};
    const p = checkDay(v, r.min, r.max);

    opts.onProblem(v ? p : null);
    if (!p) {
      opts.commit(v);
    }
  }

  return {
    /** A key was pressed in the field (not Enter). */
    keydown() {
      lastKey = now();
    },
    /** The field's value changed. */
    change(v: string) {
      last = v;
      if (now() - lastKey > keyWindow) {
        apply(v);
      } else {
        opts.onProblem(null);
      }
    },
    /** Enter, or leaving the field. */
    flush(v = last) {
      if (v) {
        apply(v);
      }
    },
  };
}

/**
 * When the first seal can exist, from the earliest data: the first full
 * UTC day is the day after `dataSince` (or that day, if the data starts
 * exactly at midnight); its seal is made within an hour after it ends.
 * Returns the end of that day in ms, or null without a start.
 */
export function firstSealAfter(dataSince: string | null | undefined): number | null {
  const t = dataSince ? Date.parse(dataSince) : NaN;

  if (isNaN(t)) {
    return null;
  }
  const start = Math.ceil(t / DAY_MS) * DAY_MS;

  return start + DAY_MS;
}
