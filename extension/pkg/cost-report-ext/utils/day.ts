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
