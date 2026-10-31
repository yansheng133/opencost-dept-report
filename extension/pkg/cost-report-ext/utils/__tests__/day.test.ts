import { describe, it, expect, jest } from '@jest/globals';
import {
  completeDay, dayLocalRange, firstSealAfter, isOngoingDay, settleDay, utcMidnightLocal
} from '../day';

describe('UTC days', () => {
  it('knows whether the day has ended', () => {
    const now = Date.parse('2026-10-31T05:00:00Z');

    expect(isOngoingDay('2026-10-31', now)).toBe(true);
    expect(isOngoingDay('2026-10-30', now)).toBe(false);
    expect(isOngoingDay('2026-11-01', now)).toBe(true);
  });

  it('shows the local equivalent (whatever the test machine zone is)', () => {
    const r = dayLocalRange('2026-10-30');
    const off = new Date(Date.parse('2026-10-30T00:00:00Z'));
    const pad = (n: number) => String(n).padStart(2, '0');

    expect(r.from).toBe(`${ pad(off.getMonth() + 1) }-${ pad(off.getDate()) } ${ pad(off.getHours()) }:${ pad(off.getMinutes()) }`);
    expect(r.to).not.toBe(r.from);
    expect(utcMidnightLocal(Date.parse('2026-10-30T13:00:00Z'))).toBe(`${ pad(off.getHours()) }:${ pad(off.getMinutes()) }`);
  });

  it('accepts only complete dates', () => {
    expect(completeDay('2026-10-01')).toBe('2026-10-01');
    for (const partial of ['0002-10-01', '0202-10-01', '2026-02-30', '2026-1-01', '', '20261001']) {
      expect(completeDay(partial)).toBeNull();
    }
  });
});

describe('settleDay', () => {
  it('queries only after the value stays, or on Enter/blur, never for partial dates', () => {
    jest.useFakeTimers();
    const done: string[] = [];
    const s = settleDay(1200, (d) => done.push(d));

    // Typing the day digit by digit: each value is "complete".
    s.type('2026-10-03');
    jest.advanceTimersByTime(500);
    s.type('2026-10-30');
    jest.advanceTimersByTime(1199);
    expect(done).toStrictEqual([]);
    jest.advanceTimersByTime(1);
    expect(done).toStrictEqual(['2026-10-30']);

    // An incomplete value cancels the pending one.
    s.type('2026-11-05');
    s.type('0002-11-05');
    jest.advanceTimersByTime(5000);
    expect(done).toStrictEqual(['2026-10-30']);

    // Enter or blur: at once, with the last value.
    s.type('2026-11-07');
    s.flush();
    expect(done).toStrictEqual(['2026-10-30', '2026-11-07']);
    jest.advanceTimersByTime(5000);
    expect(done).toHaveLength(2);
    s.flush('');
    expect(done).toHaveLength(2);
    jest.useRealTimers();
  });
});

describe('firstSealAfter', () => {
  it('is the end of the first full UTC day of data', () => {
    expect(firstSealAfter('2026-10-31T01:20:00Z')).toBe(Date.parse('2026-11-02T00:00:00Z'));
    expect(firstSealAfter('2026-10-31T00:00:00Z')).toBe(Date.parse('2026-11-01T00:00:00Z'));
    expect(firstSealAfter(null)).toBeNull();
    expect(firstSealAfter('nonsense')).toBeNull();
  });
});
