// A report as the backend returns it (docs/api.md), for the component
// smoke tests. Two departments, a gap, a storage row not measured.
import type { Report, BackendConfig } from '../../utils/types';

export const config: BackendConfig = {
  version:       '1.0.0',
  cluster:       'prod-east',
  windows:       ['1h', '24h', '7d'],
  defaultWindow: '24h',
  departments:   [{ id: 'mfg', name: '製造部' }, { id: 'rd', name: '研發部' }],
  scope:         { all: true, departments: ['mfg', 'rd'] },
  labels:        {
    department: 'cost-center', environment: 'env', environments: ['prod', 'dev']
  },
  features: {
    seals: true, volumeUsage: true, snapshotter: true, coverage: true
  },
  currency: 'TWD',
};

const at = (h: number) => new Date(Date.parse('2026-09-24T00:00:00Z') + h * 3600e3).toISOString();

export function report(over: Partial<Report> = {}): Report {
  return {
    generatedAt: at(24),
    ageSeconds:  12,
    ttlSeconds:  60,
    stale:       false,
    staleReason: null,
    windows:     ['1h', '24h', '7d'],
    scope:       { all: true, departments: ['mfg', 'rd'] },
    window:      {
      query: '24h', start: at(0), end: at(24), minutes: 1440
    },
    cluster:  'prod-east',
    coverage: {
      available:        true,
      scrapePct:        98,
      windowPct:        100,
      requestedMinutes: 1440,
      measuredMinutes:  1440,
      pct:              98,
      measuredStep:     true,
      stepSeconds:      60,
      gaps:             [{
        start: at(3), end: at(4), minutes: 60
      }],
      gapMinutes: 60
    },
    targets: [{
      job: 'opencost', expected: 1, worstHealthy: 0, degradedPct: 4.2, status: 'degraded'
    }],
    series: {
      step:        '1h',
      stepMinutes: 60,
      points:      [0, 1, 2, 3, 4].map((h) => ({
        start:   at(h),
        minutes: 60,
        depts:   h === 3 ? null : {
          mfg: {
            cpu: 0.3, ram: 0.5, cpuReq: 1, cpuUse: 0.3
          },
          rd: { cpu: 0.7, ram: null }
        }
      })),
      covered:     4,
      expected:    5,
      options:     ['5m', '10m', '30m', '1h'],
      defaultStep: '1h',
    },
    rates:        {
      cpu: 1, ram: 0.25, storage: 0.001
    },
    ratesImplied: {
      cpu: 1, ram: 0.25, storage: 0.001
    },
    rateCard:     {
      loaded:   true,
      currency: 'TWD',
      note:     'hardware amortisation',
      version:  {
        effective: '2026-01-01', cpu: 1, ram: 0.25, storage: 0.001, author: 'platform-team', reason: 'first version'
      },
      versions: [{ effective: '2026-01-01' }],
      check:    {
        cpu:     {
          configured: 1, implied: 1, status: 'ok'
        },
        ram:     {
          configured: 0.25, implied: 0.25, status: 'ok'
        },
        storage: {
          configured: 0.001, implied: 0.00105, status: 'minor', driftPct: 5, costShare: 0.1, impactPct: 0.005
        },
      },
    },
    depts: [
      {
        id: 'mfg', name: '製造部', cpu: 10, ram: 2, pv: 0.1, total: 12.1, shared: 15, cpuCoreHours: 10, ramGiBHours: 8, pvGiBHours: 100, measured: 12.1, estimated: 0, basis: 'measured'
      },
      {
        id: 'rd', name: '研發部', cpu: 20, ram: 4, pv: 0, total: 24, shared: 29, cpuCoreHours: 20, ramGiBHours: 16, pvGiBHours: 0, measured: 23, estimated: 1, basis: 'mixed'
      },
    ],
    idle:          8,
    unallocated:   { separate: 5.9, shared: 6 },
    total:         50,
    measuredTotal: 49,
    basis:         {
      estimatedTotal: 1,
      estimatedPct:   2,
      fill:           {
        byDept:              { rd: 1 },
        total:               1,
        basis:               'declared',
        snapshotCoveragePct: 99,
        notes:               [{
          start: at(3), end: at(4), code: 'no_snapshot'
        }]
      }
    },
    reconciliation: {
      allocated: 50, assets: 50.1, driftPct: -0.2, status: 'ok', byType: { Node: 49, Disk: 1.1 }
    },
    gate: {
      verdict: 'mark',
      checks:  [
        {
          code: 'has_data', value: null, unit: '', status: 'ok'
        },
        {
          code: 'coverage', value: 98, unit: '%', status: 'mark'
        },
        {
          code:   'rates',
          value:  null,
          unit:   '',
          status: 'mark',
          detail: {
            code: 'rates_minor',
            args: {
              items: [{
                resource: 'storage', driftPct: 5, costShare: 0.1, impactPct: 0.005
              }]
            }
          }
        },
      ],
      thresholds: {
        coverageOk: 99, coverageMark: 95, estimatedOk: 1, estimatedMark: 10, driftOk: 0.5, driftMark: 2, unallocatedMark: 20
      },
    },
    ccEnv:     [{
      dept: 'mfg', envs: { prod: 10, dev: 2.1 }, other: 0
    }, {
      dept: 'rd', envs: { prod: 20 }, other: 4
    }],
    workloads: [
      {
        name: 'erp', kind: 'deployment', ns: 'mfg-app', dept: 'mfg', env: 'prod', total: 8, cpuReq: 2, cpuUse: 0.05, ramReqMi: 1024, ramUseMi: 100, cpuEff: 0.025, ramEff: 0.1
      },
      {
        name: 'ci', kind: 'deployment', ns: 'rd-ci', dept: 'rd', env: null, total: 20, cpuReq: 1, cpuUse: 1.5, ramReqMi: 64, ramUseMi: 60, cpuEff: 1.5, ramEff: 0.9
      },
    ],
    unallocRows: [{
      ns: 'shop', total: 3, system: false
    }, {
      ns: 'kube-system', total: 2.9, system: true
    }],
    storage:     {
      rows: [{
        pv:    'pvc-1',
        ns:    'mfg-app',
        pvc:   'data',
        depts: [{ id: 'mfg', cost: 0.1 }, {
          id: '__unallocated__', cost: 0.0001, special: 'unallocated'
        }],
        capacityBytes: 2 * 1024 ** 3,
        usedBytes:     null,
        peakBytes:     null,
        usedPct:       null,
        over:          false,
        partial:       false,
        cost:          0.1,
        idleCost:      null,
        gibHours:      48
      }],
      total:        0.1,
      idleCost:     null,
      measuredCost: 0,
      measured:     0,
      count:        1,
      collector:    {
        installed: false, lastScan: null, error: 'not installed'
      },
    },
    ...over,
  };
}
