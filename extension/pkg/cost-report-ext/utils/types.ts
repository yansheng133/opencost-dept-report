// Shapes of the backend's answers (docs/api.md). Fields the page does not
// read are left out; numbers the backend may not know are `| null`.

export interface Scope {
  // true: every department and the cluster-wide figures (idle,
  // unallocated, reconciliation, targets).
  all: boolean;
  departments: string[];
}

export interface Department {
  id: string;
  name: string;
}

// GET config
export interface BackendConfig {
  version?: string;
  cluster?: string;
  windows: string[];
  defaultWindow: string;
  departments: Department[];
  scope: Scope;
  labels?: { department?: string; environment?: string; environments?: string[] };
  features?: { seals?: boolean; volumeUsage?: boolean; snapshotter?: boolean; coverage?: boolean };
  currency?: string | null;
}

export type Status = 'ok' | 'mark' | 'block' | 'unknown';

export interface Gap { start: string; end: string; minutes: number }

export interface Coverage {
  available: boolean;
  reason?: string;
  scrapePct?: number;
  windowPct?: number | null;
  requestedMinutes?: number | null;
  measuredMinutes?: number | null;
  pct?: number;
  measuredStep?: boolean;
  stepSeconds?: number;
  gaps?: Gap[];
  gapMinutes?: number;
}

export interface TargetRow {
  job: string;
  expected: number;
  worstHealthy: number;
  degradedPct: number;
  status: 'ok' | 'degraded';
}

export interface SeriesDept {
  cpu: number | null;
  ram: number | null;
  cpuReq?: number;
  cpuUse?: number;
}

export interface SeriesPoint {
  start: string;
  end?: string;
  minutes?: number;
  // null: no data in this segment (the line breaks there).
  depts: Record<string, SeriesDept> | null;
}

export interface Series {
  step: string;
  stepMinutes: number;
  points: SeriesPoint[];
  covered: number;
  expected: number;
  options: string[];
  defaultStep: string;
  ageSeconds?: number;
}

export interface DeptRow {
  id: string;
  name?: string;
  cpu: number;
  ram: number;
  pv: number;
  total: number;
  shared: number;
  cpuCoreHours: number;
  ramGiBHours: number;
  pvGiBHours: number;
  measured?: number;
  estimated?: number;
  basis?: 'measured' | 'mixed' | string;
}

export interface Workload {
  name: string;
  kind: string;
  ns: string;
  dept: string | null;
  env: string | null;
  total: number;
  cpuReq: number;
  cpuUse: number;
  ramReqMi: number;
  ramUseMi: number;
  cpuEff: number;
  ramEff: number;
}

// special: not a department (OpenCost's __unallocated__, __unmounted__, __idle__).
export interface StorageDept { id: string; cost: number; special?: 'unallocated' | 'unmounted' | 'idle' | null }

export interface StorageRow {
  pv: string;
  ns: string | null;
  pvc: string | null;
  depts: StorageDept[];
  capacityBytes: number | null;
  usedBytes: number | null;
  peakBytes: number | null;
  usedPct: number | null;
  over: boolean;
  partial: boolean;
  cost: number;
  idleCost: number | null;
  gibHours: number;
}

export interface Storage {
  rows: StorageRow[];
  total: number;
  idleCost: number | null;
  measuredCost: number;
  measured: number;
  count: number;
  collector?: { installed: boolean; lastScan: string | null; error: string | null; coveredPvs?: number };
}

export interface UnallocRow { ns: string; total: number; system: boolean }

export interface EnvRow {
  dept: string;
  envs: Record<string, number>;
  other: number;
}

export interface GateDetail {
  code: 'no_data' | 'rates_mismatch' | 'rates_minor' | 'rates_unknown' | 'rates_ok' | string;
  args?: Record<string, unknown>;
}

export interface GateCheck {
  code: 'has_data' | 'coverage' | 'estimated' | 'reconciliation' | 'rates' | 'unallocated' | string;
  value: number | null;
  unit: string;
  status: Status;
  detail?: GateDetail | null;
}

export interface Gate {
  verdict: 'ok' | 'mark' | 'block';
  checks: GateCheck[];
  thresholds?: Record<string, number>;
}

export interface RateCheck {
  configured: number | null;
  implied: number | null;
  status: 'ok' | 'minor' | 'mismatch' | 'unknown';
  driftPct?: number;
  costShare?: number;
  impactPct?: number;
}

export interface RateVersion {
  effective?: string;
  cpu?: number;
  ram?: number;
  storage?: number;
  author?: string;
  reason?: string;
}

export interface Rates { cpu: number | null; ram: number | null; storage: number | null }

export interface FillNote { start: string; end: string; code: 'no_snapshot' | 'unreachable' | string; detail?: string }

export interface Reconciliation {
  allocated: number;
  assets: number | null;
  driftPct: number | null;
  status: Status;
  // Why it is unknown: no asset data, or an asset total of zero or below.
  reason?: 'assets_unavailable' | 'assets_not_positive';
  // Minutes each side covers; when they differ, assets is scaled to the
  // allocation's minutes and assetsMeasured is OpenCost's own figure.
  allocatedMinutes?: number;
  assetsMinutes?: number | null;
  assetsMeasured?: number;
  scaled?: boolean;
  byType?: Record<string, number>;
}

// GET reports
export interface Report {
  generatedAt: string;
  ageSeconds: number;
  ttlSeconds: number;
  stale: boolean;
  staleReason: string | null;
  windows: string[];
  scope: Scope;
  window: { query: string; start: string | null; end: string | null; minutes: number };
  cluster: string;
  coverage: Coverage | null;
  targets: TargetRow[] | null;
  series: Series | null;
  rates: Rates;
  ratesImplied: Rates;
  rateCard: {
    loaded: boolean;
    currency: string | null;
    note: string | null;
    version: RateVersion | null;
    versions: RateVersion[];
    check: Record<string, RateCheck>;
  };
  depts: DeptRow[];
  idle: number | null;
  unallocated: { separate: number; shared: number } | null;
  total: number;
  measuredTotal: number | null;
  basis: {
    estimatedTotal: number;
    estimatedPct: number;
    fill: { byDept: Record<string, number>; total: number; basis: string; snapshotCoveragePct: number | null; notes: FillNote[] } | null;
  };
  reconciliation: Reconciliation | null;
  gate: Gate;
  ccEnv: EnvRow[];
  workloads: Workload[];
  unallocRows: UnallocRow[] | null;
  storage: Storage | null;
}

// GET seals
export interface SealRow {
  day: string;
  sealedAt?: string;
  total?: number | null;
  coveragePct?: number | null;
  estimatedPct?: number | null;
  gate?: 'ok' | 'mark' | 'block' | null;
  driftPct?: number | null;
  intact?: boolean;
  status?: string;
}

// GET periods
export interface Month {
  month: string;
  days: string[];
  total: number;
  idle: number | null;
  unallocated: number | null;
  estimated: number;
  estimatedPct: number;
  depts: Record<string, number>;
  worstGate: string;
  tampered: string[];
  minCoverage: number | null;
  gapMinutes: number;
  sealedDays: number;
  expectedDays: number;
  monthDays: number;
  missingDays: string[];
  complete: boolean;
  verdict: 'ok' | 'mark' | 'block';
  // Why the month got its verdict, translated by the page
  // (costReport.periods.reason.<code>).
  reason?: {
    code: 'tampered' | 'missing_days' | 'in_progress' | 'day_blocked' | 'day_marked' | 'all_ok' | string;
    args?: { days?: string[]; missing?: number; sealed?: number };
  } | null;
}
