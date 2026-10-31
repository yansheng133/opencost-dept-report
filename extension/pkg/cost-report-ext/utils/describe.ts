// Text for the codes the backend and utils/actions.ts return. The
// translation function is passed in, so these stay pure and testable.
import {
  n2, n4, pct, fmtBytes, fmtLocal, dayRanges
} from './format';
import type {
  Gap, GateCheck, Month, FillNote, StorageRow, Workload
} from './types';
import type { Item } from './actions';

export type Translate = (key: string, args?: Record<string, unknown>) => string;

const K = 'costReport';

export interface Context {
  t: Translate;
  // Department display name.
  name: (id: string) => string;
  // The department label key (cost-center).
  label: string;
  // Months per sample (monthFactor).
  month: number;
}

export const gapText = (t: Translate, g: Gap) => t(`${ K }.coverage.gap`, {
  start: fmtLocal(g.start, true), end: fmtLocal(g.end, true), minutes: g.minutes
});

const resourceName = (t: Translate, r: unknown) => (typeof r === 'string' ? t(`${ K }.resource.${ r }`) : String(r));

/** Name, reason, policy and detail of one billing check. */
export function gateCheckText(t: Translate, c: GateCheck, th: Record<string, number> = {}) {
  const k = `${ K }.gate.check.${ c.code }`;
  const limits: Record<string, Record<string, unknown>> = {
    coverage:       { ok: th.coverageOk, mark: th.coverageMark },
    estimated:      { ok: th.estimatedOk, mark: th.estimatedMark },
    reconciliation: { ok: th.driftOk, mark: th.driftMark },
    unallocated:    { mark: th.unallocatedMark },
  };
  let detail = '';
  const d = c.detail;

  if (d?.code === 'rates_mismatch') {
    const list = Array.isArray(d.args?.resources) ? d.args!.resources as unknown[] : [];

    detail = t(`${ K }.gate.detail.rates_mismatch`, { resources: list.map((r) => resourceName(t, r)).join(', ') });
  } else if (d?.code === 'rates_minor') {
    // api.md describes args.items[]; its example has one item's fields
    // directly in args. Both are read.
    const items = (Array.isArray(d.args?.items) ? d.args!.items : [d.args || {}]) as Record<string, unknown>[];

    detail = items.map((i) => t(`${ K }.gate.detail.rates_minor`, { ...i, resource: resourceName(t, i.resource) })).join('; ');
  } else if (d?.code) {
    detail = t(`${ K }.gate.detail.${ d.code }`);
  }

  return {
    name:   t(`${ k }.name`),
    why:    t(`${ k }.why`),
    policy: t(`${ k }.policy`, limits[c.code] || {}),
    detail,
  };
}

export function fillNoteText(t: Translate, n: FillNote): string {
  return t(`${ K }.trust.fillNote.${ n.code }`, { start: fmtLocal(n.start, true), end: fmtLocal(n.end, true) }) + (n.detail ? ` (${ n.detail })` : '');
}

/** Title and text of a next action. */
export function actionText(c: Context, item: Item): { title: string; text: string } {
  const { t } = c;
  const k = `${ K }.actions.${ item.code }`;
  const a = item.args as Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

  switch (item.code) {
  case 'labelNamespaces':
    return {
      title: t(`${ k }.title`, { count: a.namespaces.length, label: c.label }),
      text:  t(`${ k }.text`, { list: a.namespaces.map((x: { ns: string; total: number }) => `${ x.ns } (${ n4(x.total) })`).join(', ') }),
    };
  case 'platformRule':
    return {
      title: t(`${ k }.title`),
      text:  t(`${ k }.text`, {
        list: a.namespaces.join(', ') + (a.more ? t(`${ k }.more`) : ''), total: n4(a.total), pct: n2(a.pct)
      }),
    };
  case 'rightsize':
    return {
      title: t(`${ k }.title`),
      text:  t(`${ k }.text`, {
        list: a.workloads.map((w: Workload) => t(`${ k }.item`, {
          name:   `${ w.ns }/${ w.name }`,
          req:    w.cpuReq.toFixed(3),
          use:    w.cpuUse < 0.001 ? t(`${ k }.tiny`) : w.cpuUse,
          eff:    pct(w.cpuEff),
          amount: n4(w.total),
          month:  n4(w.total * c.month),
        })).join('; ')
      }),
    };
  case 'idle':
    return {
      title: t(`${ k }.title`),
      text:  t(`${ k }.text`, {
        pct: n2(a.pct), idle: n4(a.idle), month: n4(a.idle * c.month), who: t(`${ k }.${ a.shared ? 'whoShared' : 'whoSeparate' }`)
      }),
    };
  case 'gate':
    return {
      title: t(`${ k }.title`),
      text:  t(`${ k }.text`, { list: a.checks.map((x: { code: string; status: string }) => `${ t(`${ K }.gate.check.${ x.code }.name`) } (${ t(`${ K }.gate.status.${ x.status }`) })`).join(', ') }),
    };
  default:
    return { title: t(`${ k }.title`), text: t(`${ k }.text`) };
  }
}

/** Title and text of a note under the workload table. */
export function workloadNoteText(t: Translate, item: Item): { title: string; text: string } {
  const w = item.args as unknown as Workload;
  const k = `${ K }.workloads.notes.${ item.code }`;
  const args: Record<string, unknown> = {
    name: w.name, amount: n4(w.total), eff: pct(item.code === 'ramIdle' ? w.ramEff : w.cpuEff)
  };

  if (item.code === 'cpuIdle') {
    args.req = w.cpuReq;
  } else if (item.code === 'ramIdle') {
    args.req = w.ramReqMi;
    args.use = w.ramUseMi;
  }

  return { title: t(`${ k }.title`), text: t(`${ k }.text`, args) };
}

/** Title and text of a note under the storage table. */
export function storageNoteText(t: Translate, item: Item): { title: string; text: string } {
  const k = `${ K }.storage.notes.${ item.code }`;
  const a = item.args as Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
  const rowName = (r: StorageRow) => r.pvc || r.pv;
  let text: string;

  switch (item.code) {
  case 'notMeasured':
    text = t(`${ k }.text`) + (a.error ? ` (${ a.error })` : '');
    break;
  case 'someMissing':
    text = t(`${ k }.text`, { count: a.count, missing: a.missing });
    break;
  case 'emptySpace':
    text = t(`${ k }.text`, {
      paid: n4(a.paid), idle: n4(a.idle), ratio: pct(a.ratio)
    }) +
      (a.top ? ` ${ t(`${ k }.top`, {
        name: rowName(a.top), capacity: fmtBytes(a.top.capacityBytes), peak: fmtBytes(a.top.peakBytes)
      }) }` : '');
    break;
  case 'overCapacity':
    text = t(`${ k }.text`, {
      list: a.rows.map((r: StorageRow) => t(`${ k }.item`, {
        name: rowName(r), capacity: fmtBytes(r.capacityBytes), peak: fmtBytes(r.peakBytes)
      })).join('; ')
    });
    break;
  case 'lastScan':
    text = t(`${ k }.text`, { at: fmtLocal(a.at, true) });
    break;
  default:
    text = t(`${ k }.text`);
  }

  return { title: t(`${ k }.title`), text };
}

/** Why a month got its verdict. */
export function monthReasonText(t: Translate, m: Month): string {
  const r = m.reason;

  if (!r?.code) {
    return '';
  }
  const k = `${ K }.periods.reason.${ r.code }`;

  if (r.code === 'tampered') {
    return t(k, { days: (r.args?.days || []).join(', ') });
  }
  if (r.code === 'missing_days') {
    return t(k, { missing: r.args?.missing ?? m.missingDays?.length ?? 0, sealed: r.args?.sealed ?? m.sealedDays });
  }

  return t(k);
}

/** Missing days of a month folded into ranges, as text. */
export function missingText(t: Translate, days: string[]): string {
  return dayRanges(days)
    .map((r) => (r.days === 1 ? r.from : t(`${ K }.periods.range`, r)))
    .join(', ');
}
