import { describe, it, expect } from '@jest/globals';
import {
  gateCheckText, actionText, monthReasonText, missingText, storageNoteText, fillNoteText
} from '../describe';
import type { Month } from '../types';

// Translation that shows the key and its arguments.
const t = (key: string, args?: Record<string, unknown>) => (args && Object.keys(args).length ? `${ key }${ JSON.stringify(args) }` : key);

describe('gateCheckText', () => {
  it('fills the policy with the backend’s thresholds', () => {
    const x = gateCheckText(t, {
      code: 'coverage', value: 99, unit: '%', status: 'ok'
    }, { coverageOk: 99, coverageMark: 95 });

    expect(x.name).toBe('costReport.gate.check.coverage.name');
    expect(x.policy).toBe('costReport.gate.check.coverage.policy{"ok":99,"mark":95}');
    expect(x.detail).toBe('');
  });

  it('rate details: mismatch list, minor in both shapes', () => {
    const mism = gateCheckText(t, {
      code: 'rates', value: null, unit: '', status: 'block', detail: { code: 'rates_mismatch', args: { resources: ['cpu', 'ram'] } }
    });

    expect(mism.detail).toBe('costReport.gate.detail.rates_mismatch{"resources":"costReport.resource.cpu, costReport.resource.ram"}');
    const one = gateCheckText(t, {
      code: 'rates', value: null, unit: '', status: 'mark', detail: { code: 'rates_minor', args: { resource: 'storage', driftPct: 5.6 } }
    });
    const many = gateCheckText(t, {
      code: 'rates', value: null, unit: '', status: 'mark', detail: { code: 'rates_minor', args: { items: [{ resource: 'storage', driftPct: 5.6 }] } }
    });

    expect(one.detail).toBe(many.detail);
    expect(one.detail).toContain('"resource":"costReport.resource.storage"');
  });
});

describe('actionText', () => {
  const ctx = {
    t, name: (id: string) => id, label: 'cost-center', month: 30
  };

  it('lists names and amounts', () => {
    const a = actionText(ctx, { code: 'labelNamespaces', args: { namespaces: [{ ns: 'shop', total: 1.5 }] } });

    expect(a.title).toBe('costReport.actions.labelNamespaces.title{"count":1,"label":"cost-center"}');
    expect(a.text).toContain('shop (1.5000)');
    const idle = actionText(ctx, {
      code: 'idle',
      args: {
        pct: 30, idle: 2, shared: false
      }
    });

    expect(idle.text).toContain('"month":"60.0000"');
    expect(idle.text).toContain('"who":"costReport.actions.idle.whoSeparate"');
    expect(actionText(ctx, { code: 'none', args: {} }).text).toBe('costReport.actions.none.text');
  });
});

describe('month reasons', () => {
  const m = (reason: Month['reason']) => ({
    reason, missingDays: ['2026-09-01'], sealedDays: 20
  } as unknown as Month);

  it('translates the code with its arguments', () => {
    expect(monthReasonText(t, m({ code: 'tampered', args: { days: ['2026-09-02', '2026-09-03'] } }))).toBe('costReport.periods.reason.tampered{"days":"2026-09-02, 2026-09-03"}');
    expect(monthReasonText(t, m({ code: 'missing_days', args: { missing: 3, sealed: 20 } }))).toBe('costReport.periods.reason.missing_days{"missing":3,"sealed":20}');
    expect(monthReasonText(t, m({ code: 'all_ok' }))).toBe('costReport.periods.reason.all_ok');
    expect(monthReasonText(t, m(null))).toBe('');
  });

  it('missing days as ranges', () => {
    expect(missingText(t, ['2026-09-01', '2026-09-02', '2026-09-05'])).toBe('costReport.periods.range{"from":"09-01","to":"09-02","days":2}, 09-05');
  });
});

describe('notes', () => {
  it('storage and fill notes', () => {
    expect(storageNoteText(t, { code: 'notMeasured', args: { error: 'no prom' } }).text).toBe('costReport.storage.notes.notMeasured.text (no prom)');
    expect(fillNoteText(t, {
      start: '2026-09-24T00:00:00Z', end: '2026-09-24T01:00:00Z', code: 'no_snapshot'
    })).toContain('costReport.trust.fillNote.no_snapshot');
  });
});
