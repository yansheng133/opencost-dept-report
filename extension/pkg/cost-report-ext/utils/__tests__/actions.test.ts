import { describe, it, expect } from '@jest/globals';
import { nextActions, mergedWorkloads, workloadNotes, storageNotes } from '../actions';
import type { Report, Workload, StorageRow, Storage } from '../types';

const wl = (over: Partial<Workload>): Workload => ({
  name: 'w', kind: 'deployment', ns: 'n', dept: 'rd', env: 'prod', total: 1, cpuReq: 1, cpuUse: 0.5, ramReqMi: 64, ramUseMi: 32, cpuEff: 0.5, ramEff: 0.5, ...over
});

const base = (over: Partial<Report>): Report => ({
  total:       10,
  idle:        1,
  unallocRows: [],
  workloads:   [],
  gate:        { verdict: 'ok', checks: [] },
  ...over,
} as unknown as Report);

describe('nextActions', () => {
  it('empty cluster: only "check the data"', () => {
    expect(nextActions(base({ total: 0, idle: 5 }), false).map((a) => a.code)).toStrictEqual(['noData']);
  });

  it('separates application namespaces from platform ones', () => {
    const r = base({
      unallocRows: [
        {
          ns: 'shop', total: 3, system: false
        }, {
          ns: 'kube-system', total: 2, system: true
        },
        {
          ns: 'cattle-system', total: 1, system: true
        }
      ]
    });
    const a = nextActions(r, false);

    expect(a.map((x) => x.code)).toStrictEqual(['labelNamespaces', 'platformRule']);
    expect(a[0].args.namespaces).toStrictEqual([{ ns: 'shop', total: 3 }]);
    expect(a[1].args).toMatchObject({
      namespaces: ['kube-system', 'cattle-system'], total: 3, pct: 30
    });
  });

  it('idle above 20%, and failed gate checks', () => {
    const r = base({
      idle: 3,
      gate: {
        verdict: 'block',
        checks:  [{
          code: 'coverage', status: 'block', value: 80, unit: '%'
        }, {
          code: 'rates', status: 'ok', value: null, unit: ''
        }]
      }
    });
    const a = nextActions(r, true);

    expect(a.map((x) => x.code)).toStrictEqual(['idle', 'gate']);
    expect(a[0].args).toMatchObject({ pct: 30, shared: true });
    expect(a[1].args.checks).toStrictEqual([{ code: 'coverage', status: 'block' }]);
  });

  it('a scoped view (idle and unallocated null) still works', () => {
    expect(nextActions(base({ idle: null, unallocRows: null }), false).map((x) => x.code)).toStrictEqual(['none']);
  });

  it('rightsizing lists merged workloads, biggest first', () => {
    const r = base({
      workloads: [wl({
        name: 'a', total: 1, cpuEff: 0.05
      }), wl({
        name: 'a', total: 1, cpuEff: 0.15
      }), wl({
        name: 'b', total: 5, cpuEff: 0.9
      })]
    });
    const a = nextActions(r, false);

    expect(a[0].code).toBe('rightsize');
    expect((a[0].args.workloads as Workload[]).map((w) => [w.name, w.total, Number(w.cpuEff.toFixed(2))])).toStrictEqual([['a', 2, 0.1]]);
  });
});

describe('mergedWorkloads', () => {
  it('weights by cost', () => {
    const m = mergedWorkloads([wl({
      total: 3, cpuReq: 1, cpuEff: 0.1
    }), wl({
      total: 1, cpuReq: 2, cpuEff: 0.5
    })]);

    expect(m).toHaveLength(1);
    expect(m[0].cpuReq).toBeCloseTo(1.25);
    expect(m[0].cpuEff).toBeCloseTo(0.2);
  });
});

describe('workloadNotes', () => {
  it('picks the notable ones', () => {
    const n = workloadNotes([wl({
      name: 'idle', cpuEff: 0.01, total: 3
    }), wl({
      name: 'mem', ramReqMi: 512, ramEff: 0.1
    }), wl({ name: 'hot', cpuEff: 1.4 })]);

    // Memory is used (50%): only the CPU part is idle. No standby example
    // (no stateful or per-node workload with low usage): no such note.
    expect(n.map((x) => [x.code, x.args.name])).toStrictEqual([['cpuIdleOnly', 'idle'], ['ramIdle', 'mem'], ['over', 'hot']]);
    expect(workloadNotes([])).toStrictEqual([]);
  });

  it('says "reserved capacity" only when both resources idle, and gives a low-usage standby example', () => {
    const n = workloadNotes([wl({
      name: 'idle', cpuEff: 0.01, ramEff: 0.05, total: 3
    }), wl({
      name: 'db', kind: 'statefulset', cpuEff: 0.05, total: 2
    }), wl({
      name: 'busy-db', kind: 'statefulset', cpuEff: 0.9, total: 5
    })]);

    expect(n.map((x) => [x.code, x.args.name])).toStrictEqual([['cpuIdle', 'idle'], ['standby', 'db']]);
  });
});

describe('storageNotes', () => {
  const row = (over: Partial<StorageRow>): StorageRow => ({
    pv: 'pv', ns: 'n', pvc: 'c', depts: [], capacityBytes: 100, usedBytes: 10, peakBytes: 20, usedPct: 0.2, over: false, partial: false, cost: 1, idleCost: 0.8, gibHours: 1, ...over
  });

  it('says when nothing is measured, never zero', () => {
    const n = storageNotes({
      collector: {
        installed: false, lastScan: null, error: 'x'
      }
    } as Storage, [row({ idleCost: null })]);

    expect(n.map((x) => x.code)).toStrictEqual(['notMeasured']);
  });

  it('measured: empty space, over capacity, partial, last scan', () => {
    const s = {
      measured:  1,
      count:     2,
      collector: {
        installed: true, lastScan: '2026-09-24T00:00:00Z', error: null
      }
    } as Storage;
    const n = storageNotes(s, [row({}), row({
      pv: 'big', over: true, partial: true, idleCost: 0
    })]);

    expect(n.map((x) => x.code)).toStrictEqual(['someMissing', 'emptySpace', 'overCapacity', 'partial', 'lastScan']);
    expect(n[1].args).toMatchObject({
      paid: 2, idle: 0.8, ratio: 0.4
    });
  });
});
