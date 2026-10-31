import { describe, it, expect } from '@jest/globals';
import { completeDay, dayLocalRange, isOngoingDay, utcMidnightLocal } from '../day';

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
