import { describe, it, expect } from '@jest/globals';
import {
  n4, pct, share, fmtCores, fmtMiB, pctValue, fmtBytes, fmtLocal, stepParts, monthFactor, dayRanges, capRows
} from '../format';

describe('usage', () => {
  it('fixed decimals; "<" only for a used-but-rounded zero', () => {
    expect(fmtCores(0.01)).toBe('0.010');
    expect(fmtCores(0.026)).toBe('0.026');
    expect(fmtCores(0, 0.003)).toBe('<0.001');
    expect(fmtCores(0, 0)).toBe('0.000');
    expect(fmtCores(0, null)).toBe('0.000');
    expect(fmtMiB(12)).toBe('12');
    expect(fmtMiB(0, 0.01)).toBe('<1');
    expect(fmtMiB(0)).toBe('0');
  });
});

describe('numbers', () => {
  it('formats', () => {
    expect(n4(1.23456)).toBe('1.2346');
    expect(n4(null)).toBe('0.0000');
    // One rule for every percentage: two decimals, '<0.01%' for tiny ones.
    expect(pct(0.1234)).toBe('12.34%');
    expect(pctValue(0.004)).toBe('<0.01%');
    expect(pctValue(0)).toBe('0.00%');
    expect(share(1, 0)).toBe('—');
    expect(share(1, 4)).toBe('25.00%');
    expect(pctValue(null)).toBe('—');
    expect(pctValue(99.6)).toBe('99.60%');
  });

  it('sizes in powers of two', () => {
    expect(fmtBytes(null)).toBe('—');
    expect(fmtBytes(512)).toBe('512 B');
    expect(fmtBytes(1536)).toBe('1.50 KiB');
    expect(fmtBytes(2 * 1024 ** 3)).toBe('2.00 GiB');
    expect(fmtBytes(150 * 1024 ** 2)).toBe('150 MiB');
  });

  it('monthly factor and steps', () => {
    expect(monthFactor(1440)).toBe(30);
    expect(monthFactor(0)).toBe(1);
    expect(stepParts('10m')).toStrictEqual({ n: '10', unit: 'm' });
    expect(stepParts('x')).toStrictEqual({ n: 'x', unit: '' });
  });
});

describe('fmtLocal', () => {
  it('uses the local time zone, not the string', () => {
    const iso = '2026-09-24T05:07:00Z';
    const d = new Date(iso);
    const hm = `${ String(d.getHours()).padStart(2, '0') }:${ String(d.getMinutes()).padStart(2, '0') }`;

    expect(fmtLocal(iso)).toBe(hm);
    expect(fmtLocal(iso, true).endsWith(hm)).toBe(true);
    expect(fmtLocal('nonsense')).toBe('nonsense');
    expect(fmtLocal(null)).toBe('—');
  });
});

describe('dayRanges', () => {
  it('folds consecutive days, across month ends', () => {
    expect(dayRanges([])).toStrictEqual([]);
    expect(dayRanges(['2026-09-03', '2026-09-01', '2026-09-02', '2026-09-05'])).toStrictEqual([
      {
        from: '09-01', to: '09-03', days: 3
      }, {
        from: '09-05', to: '09-05', days: 1
      }
    ]);
    expect(dayRanges(['2026-09-30', '2026-10-01'])).toStrictEqual([{
      from: '09-30', to: '10-01', days: 2
    }]);
  });
});

describe('capRows', () => {
  it('hides rows beyond the limit, but not a single one', () => {
    const rows = [1, 2, 3, 4, 5];

    expect(capRows(rows, 3, false)).toStrictEqual({ shown: [1, 2, 3], hidden: 2 });
    expect(capRows(rows, 4, false)).toStrictEqual({ shown: rows, hidden: 0 });
    expect(capRows(rows, 3, true)).toStrictEqual({ shown: rows, hidden: 0 });
  });
});
