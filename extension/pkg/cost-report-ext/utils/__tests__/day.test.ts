import { describe, it, expect } from '@jest/globals';
import {
  completeDay, dayLocalRange, checkDay, dayField, firstSealAfter, isOngoingDay, utcMidnightLocal
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

describe('dayField', () => {
  it('applies picker choices at once, typed dates only on Enter or blur', () => {
    let clock = 10_000;
    const done: string[] = [];
    const problems: unknown[] = [];
    const f = dayField({
      commit:    (d) => done.push(d),
      onProblem: (p) => problems.push(p),
      range:     () => ({ min: '2026-10-31', max: '2026-11-05' }),
      now:       () => clock,
    });

    // Picker: no key pressed before the change.
    f.change('2026-11-01');
    expect(done).toStrictEqual(['2026-11-01']);
    // Typing 2026 → 11 → 0 → 3, pausing as long as you like: nothing yet.
    for (const v of ['2026-11-01', '2026-11-00', '2026-11-03']) {
      f.keydown();
      clock += 10;
      f.change(v);
      clock += 3000;
    }
    expect(done).toStrictEqual(['2026-11-01']);
    // Enter (or blur) applies the last value.
    f.flush();
    expect(done).toStrictEqual(['2026-11-01', '2026-11-03']);
    // Out of range or invalid: a reason, no query.
    f.flush('2026-10-30');
    f.flush('2026-11-09');
    f.flush('0002-11-03');
    expect(done).toHaveLength(2);
    expect(problems.slice(-3)).toStrictEqual(['before', 'after', 'invalid']);
  });

  it('checks the range', () => {
    expect(checkDay('2026-11-01', '2026-10-31', '2026-11-05')).toBeNull();
    expect(checkDay('2026-11-01')).toBeNull();
    expect(checkDay('2026-02-30')).toBe('invalid');
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
