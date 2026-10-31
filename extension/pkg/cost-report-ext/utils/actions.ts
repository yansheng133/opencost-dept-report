// Observations and next steps computed from the report itself, with the
// actual names and amounts (generic advice is not worth a section). Each
// item is a code and arguments; the page translates them
// (costReport.actions.*, costReport.workloads.notes.*,
// costReport.storage.notes.*).
import type { Report, Workload, Storage, StorageRow } from './types';

export interface Item {
  code: string;
  args: Record<string, unknown>;
}

export const MAX_LISTED = 3;

/**
 * What to do next, most important first. `idleShared` is the page's
 * idle toggle (who currently absorbs idle cost).
 */
export function nextActions(r: Report, idleShared: boolean): Item[] {
  if (!r.total) {
    // An empty cluster is most likely "just installed" or "wrong period",
    // not "nothing to do"; every ratio below would look perfect.
    return [{ code: 'noData', args: {} }];
  }
  const out: Item[] = [];
  const rows = r.unallocRows || [];
  const apps = rows.filter((x) => !x.system);
  const sys = rows.filter((x) => x.system);
  const sysTotal = sys.reduce((a, x) => a + x.total, 0);

  if (apps.length) {
    out.push({ code: 'labelNamespaces', args: { namespaces: apps.slice(0, MAX_LISTED).map((x) => ({ ns: x.ns, total: x.total })) } });
  }
  if (sysTotal > 0) {
    // Platform components are shared by everyone: they need a rule, not a
    // department label.
    out.push({
      code: 'platformRule',
      args: {
        namespaces: sys.slice(0, MAX_LISTED).map((x) => x.ns), more: sys.length > MAX_LISTED, total: sysTotal, pct: sysTotal / r.total * 100
      }
    });
  }
  const waste = mergedWorkloads(r.workloads || [])
    .filter((w) => w.cpuReq > 0 && w.cpuEff < 0.2)
    .sort((a, b) => b.total - a.total)
    .slice(0, MAX_LISTED);

  if (waste.length) {
    out.push({ code: 'rightsize', args: { workloads: waste } });
  }
  if (r.idle !== null && r.idle !== undefined) {
    const idlePct = r.idle / r.total * 100;

    if (idlePct > 20) {
      out.push({
        code: 'idle',
        args: {
          pct: idlePct, idle: r.idle, shared: idleShared
        }
      });
    }
  }
  const bad = (r.gate?.checks || []).filter((c) => c.status === 'block' || c.status === 'mark');

  if (bad.length) {
    out.push({ code: 'gate', args: { checks: bad.map((c) => ({ code: c.code, status: c.status })) } });
  }
  if (!out.length) {
    out.push({ code: 'none', args: {} });
  }

  return out;
}

/**
 * Workloads merged by namespace/name. OpenCost returns two rows for a
 * workload whose labels changed during the period (before and after); the
 * same name twice in a list of advice only confuses. Efficiency and
 * requests are weighted by cost.
 */
export function mergedWorkloads(list: Workload[]): Workload[] {
  const m = new Map<string, Workload & { cpuW: number; effW: number }>();

  list.forEach((w) => {
    const k = `${ w.ns }/${ w.name }`;
    const cur = m.get(k) || {
      ...w, total: 0, cpuW: 0, effW: 0
    };

    cur.total += w.total || 0;
    cur.cpuW += (w.cpuReq || 0) * (w.total || 0);
    cur.effW += (w.cpuEff || 0) * (w.total || 0);
    m.set(k, cur);
  });

  return [...m.values()].map(({ cpuW, effW, ...w }) => ({
    ...w,
    cpuEff: w.total ? effW / w.total : 0,
    cpuReq: w.total ? cpuW / w.total : 0,
  }));
}

/** What stands out in the workload table (under the filter). */
export function workloadNotes(list: Workload[]): Item[] {
  const out: Item[] = [];
  const top = <T>(xs: T[], by: (x: T) => number) => [...xs].sort((a, b) => by(b) - by(a))[0];
  const cpuWaste = top(list.filter((w) => w.cpuReq > 0 && w.cpuEff < 0.1), (w) => w.total);
  const ramWaste = top(list.filter((w) => w.ramReqMi >= 128 && w.ramEff < 0.2), (w) => w.ramReqMi);
  const busy = top(list.filter((w) => w.cpuEff >= 0.5), (w) => w.cpuEff);
  const over = top(list.filter((w) => w.cpuEff > 1), (w) => w.cpuEff);

  if (cpuWaste) {
    out.push({ code: 'cpuIdle', args: { ...cpuWaste } });
  }
  if (ramWaste) {
    out.push({ code: 'ramIdle', args: { ...ramWaste } });
  }
  // Low usage is not the same as waste: some services do work, some use
  // more than they request (and should request more).
  if (busy) {
    out.push({ code: 'busy', args: { ...busy } });
  }
  if (over) {
    out.push({ code: 'over', args: { ...over } });
  }

  return out;
}

/** What to say under the storage table (rows already filtered). */
export function storageNotes(s: Storage | null | undefined, rows: StorageRow[]): Item[] {
  const out: Item[] = [];
  const col = s?.collector;

  if (!col?.installed) {
    // Not measured is not zero: say it.
    out.push({ code: 'notMeasured', args: { error: col?.error || '' } });
  } else if (s && s.measured < s.count) {
    out.push({ code: 'someMissing', args: { count: s.count, missing: s.count - s.measured } });
  }
  const vis = rows.filter((r) => r.idleCost !== null);
  const idle = vis.reduce((a, r) => a + (r.idleCost || 0), 0);
  const paid = vis.reduce((a, r) => a + r.cost, 0);

  if (vis.length && paid > 0) {
    const t = [...vis].sort((a, b) => (b.idleCost || 0) - (a.idleCost || 0))[0];

    out.push({
      code: 'emptySpace',
      args: {
        paid, idle, ratio: idle / paid, top: t && (t.idleCost || 0) > 0 ? t : null
      }
    });
  }
  const over = rows.filter((r) => r.over);

  if (over.length) {
    out.push({ code: 'overCapacity', args: { rows: over } });
  }
  if (rows.some((r) => r.partial)) {
    out.push({ code: 'partial', args: {} });
  }
  if (col?.lastScan) {
    out.push({ code: 'lastScan', args: { at: col.lastScan } });
  }

  return out;
}
