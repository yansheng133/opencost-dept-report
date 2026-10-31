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

/**
 * The date field changes the period only on Enter, on leaving the field,
 * or after the value has stayed unchanged for `delay` ms. Other parts of
 * the date are prefilled, so typing one digit already makes a complete
 * date (2026-10-0 → 2026-10-03); it must not be queried while typing.
 * An incomplete value cancels anything pending.
 */
export function settleDay(delay: number, commit: (day: string) => void, timers: {
  set: (fn: () => void, ms: number) => unknown;
  clear: (h: unknown) => void;
} = { set: (fn, ms) => setTimeout(fn, ms), clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>) }) {
  let handle: unknown = null;
  let last = '';

  const cancel = () => {
    if (handle !== null) {
      timers.clear(handle);
      handle = null;
    }
  };

  return {
    /** A new value while typing. */
    type(v: string) {
      cancel();
      last = v;
      const d = completeDay(v);

      if (d) {
        handle = timers.set(() => {
          handle = null;
          commit(d);
        }, delay);
      }
    },
    /** Enter or leaving the field: now, if complete. */
    flush(v = last) {
      cancel();
      const d = completeDay(v);

      if (d) {
        commit(d);
      }
    },
    cancel,
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
