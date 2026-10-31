import { describe, it, expect } from '@jest/globals';
import {
  deptIndex, visibleParts, filterWorkloads, filterStorage, filterEnv, validFilter, monthDepts, PALETTE, UNALLOCATED_COLOR
} from '../scope';
import type { Report } from '../types';

const report = (over: Partial<Report>): Report => ({
  scope:          { all: true, departments: [] },
  idle:           1,
  unallocated:    { separate: 1, shared: 1 },
  reconciliation: {
    allocated: 1, assets: 1, driftPct: 0, status: 'ok'
  },
  targets:     [],
  unallocRows: [],
  basis:       {
    estimatedTotal: 0, estimatedPct: 0, fill: null
  },
  depts: [],
  ...over,
} as unknown as Report);

describe('deptIndex', () => {
  it('names from config, colors by config order, unknown IDs after them', () => {
    const d = deptIndex([{ id: 'mfg', name: '製造部' }, { id: 'rd', name: '' }]);

    expect(d.name('mfg')).toBe('製造部');
    expect(d.name('rd')).toBe('rd');
    expect(d.name('x')).toBe('x');
    expect(d.color('rd')).toBe(PALETTE[1]);
    expect(d.color('new')).toBe(PALETTE[2]);
    expect(d.color('new')).toBe(PALETTE[2]);
    expect(d.color(null)).toBe(UNALLOCATED_COLOR);
  });
});

describe('visibleParts', () => {
  it('shows cluster-wide parts only with full scope and data', () => {
    expect(visibleParts(report({}))).toMatchObject({
      idle: true, unallocated: true, reconciliation: true, targets: true, fill: false
    });
    const scoped = visibleParts(report({ scope: { all: false, departments: ['rd'] }, idle: null }));

    expect(Object.values(scoped).some(Boolean)).toBe(false);
    // even if a value slipped through, a scoped view does not show it
    expect(visibleParts(report({ scope: { all: false, departments: [] } })).idle).toBe(false);
    expect(visibleParts(report({ idle: null })).idle).toBe(false);
  });
});

describe('filters', () => {
  it('filters rows by department', () => {
    const w = [{ dept: 'rd' }, { dept: null }, { dept: 'it' }] as never[];

    expect(filterWorkloads(w, 'all')).toHaveLength(3);
    expect(filterWorkloads(w, 'rd')).toHaveLength(1);
    expect(filterStorage([{ depts: [{ id: 'rd' }, { id: 'it' }] }, { depts: [{ id: 'it' }] }] as never[], 'rd')).toHaveLength(1);
    expect(filterEnv([{
      dept: 'rd', envs: {}, other: 0
    }], 'it')).toHaveLength(0);
  });

  it('drops a filter whose department is gone', () => {
    const r = report({ depts: [{ id: 'rd' }] as never[] });

    expect(validFilter('rd', r)).toBe('rd');
    expect(validFilter('it', r)).toBe('all');
    // A configured department is kept even when this period has no rows for it.
    expect(validFilter('it', r, ['rd', 'it'])).toBe('it');
  });

  it('month departments, largest first', () => {
    expect(monthDepts({ depts: { a: 1, b: 3 } } as never)).toStrictEqual([{ id: 'b', total: 3 }, { id: 'a', total: 1 }]);
  });
});
