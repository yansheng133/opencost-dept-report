/**
 * @jest-environment jsdom
 */
// Smoke tests: mount the pages against a fake backend and check that the
// sections render with the data and that the scope is respected. They
// catch template mistakes the build does not (the build does not run the
// templates).
import {
  describe, it, expect, beforeEach, jest
} from '@jest/globals';
import { mount, flushPromises } from '@vue/test-utils';
import { config, report } from './fixture';
import { resetBackend } from '../../composables/useBackend';
import Report from '../../pages/Report.vue';
import Periods from '../../pages/Periods.vue';

const mockReplace = jest.fn(() => Promise.resolve());
let mockQuery: Record<string, string> = {};

jest.mock('vue-router', () => ({ useRoute: () => ({ query: mockQuery }), useRouter: () => ({ replace: mockReplace }) }));
jest.mock('vuex', () => ({ useStore: () => ({}) }));

const GV = '/apis/api.costreport.yansheng133.github.io/v1alpha1';

// Answers of the fake Rancher: one cluster running cost-report.
function serve(answers: Record<string, [number, unknown]>) {
  const asked: string[] = [];

  (global as unknown as { fetch: unknown }).fetch = jest.fn(async(url: string) => {
    asked.push(url);
    const path = url.replace(/^\/k8s\/clusters\/c-1/, '');
    const hit: [number, unknown] = url === '/v1/management.cattle.io.clusters' ? [200, { data: [{ id: 'c-1', spec: { displayName: 'prod' } }] }] : answers[path.split('?')[0]] || [404, {}];

    return {
      ok: hit[0] < 300, status: hit[0], json: async() => hit[1]
    };
  });

  return asked;
}

async function mountPage(name: 'Report' | 'Periods') {
  // The cluster search is shared between pages; start each test afresh.
  resetBackend();
  const Page = name === 'Report' ? Report : Periods;
  const w = mount(Page, { attachTo: document.body, global: { stubs: { 'router-link': { template: '<a><slot /></a>' } } } });

  for (let i = 0; i < 5; i++) {
    await flushPromises();
  }

  return w;
}

beforeEach(() => {
  mockQuery = {};
  mockReplace.mockClear();
  window.localStorage.clear();
  document.body.innerHTML = '';
});

describe('部門成本', () => {
  it('renders every section with full scope', async() => {
    const asked = serve({
      [GV]: [200, {}], [`${ GV }/config`]: [200, config], [`${ GV }/reports`]: [200, report()]
    });
    const w = await mountPage('Report');
    const html = w.html();

    expect(asked).toContain(`/k8s/clusters/c-1${ GV }/reports`);
    for (const id of ['gate', 'dept', 'usage', 'env', 'workloads', 'storage', 'unalloc', 'trust', 'next', 'rates']) {
      expect(w.find(`#${ id }`).exists()).toBe(true);
    }
    // names from config, the backend's checks translated by code
    expect(html).toContain('製造部');
    expect(html).toContain('costReport.gate.check.coverage.policy{\\"ok\\":99,\\"mark\\":95}');
    expect(html).toContain('costReport.gate.detail.rates_minor');
    // a gap breaks the line: two paths per department with data on both sides
    expect(w.findAll('#usage path.cr-path').length).toBeGreaterThanOrEqual(2);
    // not measured is not zero
    expect(html).toContain('costReport.storage.notMeasured');
    // cluster-wide figures shown
    expect(html).toContain('costReport.totals.idle');
    expect(html).toContain('costReport.trust.targetsTitle');
    // the formula comes from the rates in effect
    expect(html).toContain('costReport.dept.formulaText{"cpu":"1","ram":"0.25","storage":"0.001"}');
    // no demo narratives
    expect(html).not.toMatch(/示意|demo|你要做的決定|shared-tools/);
    w.unmount();
  });

  it('a department view hides cluster-wide parts and says which departments', async() => {
    const scoped = report({
      scope: { all: false, departments: ['rd'] }, idle: null, unallocated: null, measuredTotal: null, reconciliation: null, targets: null, unallocRows: null, total: 24,
    });

    scoped.depts = scoped.depts.filter((d) => d.id === 'rd');
    scoped.basis.fill = null;
    serve({
      [GV]:               [200, {}],
      [`${ GV }/config`]: [200, {
        ...config, scope: scoped.scope, departments: [{ id: 'rd', name: '研發部' }]
      }],
      [`${ GV }/reports`]: [200, scoped]
    });
    const w = await mountPage('Report');
    const html = w.html();

    expect(html).toContain('costReport.scope.some{"names":"研發部"}');
    expect(w.find('#unalloc').exists()).toBe(false);
    expect(html).not.toContain('costReport.totals.idle');
    expect(html).not.toContain('costReport.trust.targetsTitle');
    expect(html).not.toContain('costReport.trust.recon');
    expect(html).toContain('costReport.totals.totalScoped');
    w.unmount();
  });

  it('filtering a department marks the cluster-wide sections and says so', async() => {
    serve({
      [GV]: [200, {}], [`${ GV }/config`]: [200, config], [`${ GV }/reports`]: [200, report()]
    });
    const w = await mountPage('Report');

    expect(w.html()).not.toContain('costReport.report.clusterWide<');
    await w.find('input[type="radio"][value="rd"]').setValue(true);
    await flushPromises();
    const html = w.html();

    expect(html).toContain('costReport.scope.filtered{"name":"研發部"}');
    // Gate, unallocated, trust and actions carry the badge (rates follow the filter).
    // Plus the cluster-wide total in the rates footer.
    expect(w.findAll('.cr-tag-scope').length).toBe(5);
    // Shared idle gets its own column so the row adds up.
    await w.find('input[type="radio"][value="true"]').setValue(true);
    await flushPromises();
    expect(w.html()).toContain('costReport.dept.col.sharedIdle');
    w.unmount();
  });

  it('a period without data: one empty card, no sections of zeros', async() => {
    mockQuery = { day: '2026-10-30' };
    const none = report();

    none.gate = {
      verdict:    'block',
      thresholds: {},
      checks:     [{
        code: 'has_data', value: null, unit: '', status: 'block', detail: { code: 'no_data' }
      }, {
        code: 'unallocated', value: 0, unit: '%', status: 'unknown'
      }]
    };
    serve({
      [GV]: [200, {}], [`${ GV }/config`]: [200, config], [`${ GV }/reports`]: [200, none]
    });
    const w = await mountPage('Report');
    const html = w.html();

    expect(html).toContain('costReport.empty.titleDay{"day":"2026-10-30"}');
    expect(html).toContain('costReport.report.dayPeriod{"day":"2026-10-30","from":');
    expect(html).toContain('costReport.report.pastDay');
    for (const id of ['gate', 'dept', 'usage', 'trust', 'rates']) {
      expect(w.find(`#${ id }`).exists()).toBe(false);
    }
    w.unmount();
  });

  it('typing a date survives the ticking age and only Enter applies it', async() => {
    serve({
      [GV]: [200, {}], [`${ GV }/config`]: [200, config], [`${ GV }/reports`]: [200, report()]
    });
    const w = await mountPage('Report');
    const input = w.find('input[type="date"]');
    const el = input.element as HTMLInputElement;

    await input.trigger('focus');
    await input.trigger('keydown', { key: '2' });
    el.value = '2026-09-23';
    await input.trigger('input');
    await input.trigger('change');
    // The age in the header ticks every second; the page must not reset the field.
    await new Promise((resolve) => setTimeout(resolve, 1100));
    await flushPromises();
    expect((w.find('input[type="date"]').element as HTMLInputElement).value).toBe('2026-09-23');
    expect(mockReplace).not.toHaveBeenCalledWith({ query: { day: '2026-09-23' } });
    await input.trigger('keydown', { key: 'Enter' });
    expect(mockReplace).toHaveBeenCalledWith({ query: { day: '2026-09-23' } });
    w.unmount();
  });

  it('an unusable date reverts the field and keeps an inline hint until fixed; leaving the field applies a valid one', async() => {
    serve({
      [GV]: [200, {}], [`${ GV }/config`]: [200, { ...config, dataSince: '2026-09-21T10:27:00Z' }], [`${ GV }/reports`]: [200, report()]
    });
    const w = await mountPage('Report');
    const input = w.find('input[type="date"]');
    const el = input.element as HTMLInputElement;

    await input.trigger('focus');
    await input.trigger('keydown', { key: '2' });
    el.value = '2026-01-01';
    await input.trigger('input');
    await input.trigger('keydown', { key: 'Enter' });
    expect(w.find('.cr-day-hint').text()).toContain('costReport.dept.dayProblem.before');
    expect(input.attributes('aria-invalid')).toBe('true');
    // The field shows the period in use again (live window: empty).
    expect((w.find('input[type="date"]').element as HTMLInputElement).value).toBe('');
    // Still there while the user types; it is not a disappearing tooltip.
    el.value = '2026-09-2';
    await input.trigger('input');
    expect(w.find('.cr-day-hint').exists()).toBe(true);
    // Tab away with a valid date: applied at once, hint gone.
    el.value = '2026-09-23';
    await input.trigger('input');
    await input.trigger('blur');
    expect(mockReplace).toHaveBeenCalledWith({ query: { day: '2026-09-23' } });
    expect(w.find('.cr-day-hint').exists()).toBe(false);
    w.unmount();
  });

  it('the date hint goes away when the reader moves on: another window, or a valid date typed', async() => {
    serve({
      [GV]: [200, {}], [`${ GV }/config`]: [200, { ...config, dataSince: '2026-09-21T10:27:00Z' }], [`${ GV }/reports`]: [200, report()]
    });
    const w = await mountPage('Report');
    const reject = async() => {
      const input = w.find('input[type="date"]');
      const el = input.element as HTMLInputElement;

      await input.trigger('focus');
      await input.trigger('keydown', { key: '2' });
      el.value = '2026-01-01';
      await input.trigger('input');
      await input.trigger('keydown', { key: 'Enter' });
      expect(w.find('.cr-day-hint').exists()).toBe(true);

      return { input, el };
    };

    // Picking a window.
    await reject();
    await w.find('input[name="cr-window"][value="1h"]').setValue(true);
    expect(w.find('.cr-day-hint').exists()).toBe(false);
    expect(w.find('input[type="date"]').attributes('aria-invalid')).toBeUndefined();
    // Typing a usable date.
    const { input, el } = await reject();

    el.value = '2026-09-22';
    await input.trigger('input');
    expect(w.find('.cr-day-hint').exists()).toBe(false);
    // Changing the department filter.
    await reject();
    await w.find('#dept').exists();
    await w.find('input[type="radio"][value="rd"]').setValue(true);
    expect(w.find('.cr-day-hint').exists()).toBe(false);
    w.unmount();
  });

  it('a table inside a scrolling wrapper does not get a sticky header (CSS contract)', async() => {
    // jsdom has no layout, so the rule itself is checked: a sticky header in
    // a scrolling wrapper sticks to the wrapper and covered the totals row.
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    const css: string = require('fs').readFileSync(require('path').join(__dirname, '../../styles/report.scss'), 'utf8');

    expect(css).toMatch(/\.cr-table-wrap\.cr-scroll-x \.cr-table\.cr-table thead th \{\s*position: static;/);
    expect(css).toMatch(/@media \(max-width: 1100px\) \{\s*\.cr-table-wrap \.cr-table\.cr-table thead th \{\s*position: static;/);
    serve({
      [GV]: [200, {}], [`${ GV }/config`]: [200, config], [`${ GV }/reports`]: [200, report()]
    });
    const w = await mountPage('Report');

    // The department table is the one in a scrolling wrapper.
    expect(w.find('#dept .cr-table-wrap.cr-scroll-x table.cr-dept-table').exists()).toBe(true);
    expect(w.findAll('.cr-table-wrap.cr-scroll-x').length).toBe(1);
    w.unmount();
  });

  it('the whole section title folds the section', async() => {
    serve({
      [GV]: [200, {}], [`${ GV }/config`]: [200, config], [`${ GV }/reports`]: [200, report()]
    });
    const w = await mountPage('Report');
    const btn = w.find('#workloads h2 button.cr-fold-btn');

    expect(btn.attributes('aria-expanded')).toBe('true');
    expect(btn.text()).toContain('costReport.workloads.title');
    await btn.trigger('click');
    expect(w.find('#workloads h2 button.cr-fold-btn').attributes('aria-expanded')).toBe('false');
    w.unmount();
  });

  it('no department: a friendly empty state, no report request', async() => {
    const asked = serve({
      [GV]:               [200, {}],
      [`${ GV }/config`]: [200, {
        ...config, departments: [], scope: { all: false, departments: [] }
      }]
    });
    const w = await mountPage('Report');

    expect(w.html()).toContain('costReport.target.noDepartment');
    expect(asked.some((u) => u.includes('/reports'))).toBe(false);
    w.unmount();
  });

  it('a failed report says the figures are missing, not zero', async() => {
    serve({
      [GV]: [200, {}], [`${ GV }/config`]: [200, config], [`${ GV }/reports`]: [503, { kind: 'Status', error: { code: 'not_ready', message: 'waiting' } }]
    });
    const w = await mountPage('Report');

    expect(w.html()).toContain('costReport.report.loadFailedHint');
    expect(w.html()).toContain('costReport.errors.not_ready');
    w.unmount();
  });

  it('a chosen step is asked from series; 429 keeps it pending with a countdown', async() => {
    mockQuery = { step: '10m' };
    const asked = serve({
      [GV]: [200, {}], [`${ GV }/config`]: [200, config], [`${ GV }/reports`]: [200, report()], [`${ GV }/series`]: [429, { kind: 'Status', error: { code: 'series_budget', retryAfterSeconds: 12 } }]
    });
    const w = await mountPage('Report');

    expect(asked.some((u) => u.includes('/series?window=24h&step=10m') || u.includes('/series?step=10m'))).toBe(true);
    // Refused for now: the chosen step stays pending with a countdown, no jump back.
    const usageText = w.find('#usage').text();

    expect(usageText).toContain('costReport.usage.waitStep');
    expect(usageText).toContain('"seconds":12');
    expect((w.find('#usage input[type="radio"][value="10m"]').element as HTMLInputElement).checked).toBe(true);
    expect(w.findAll('#usage path.cr-path').length).toBeGreaterThanOrEqual(2);
    w.unmount();
  });

  it('a step this period does not offer is dropped from the URL', async() => {
    mockQuery = { step: '7m' };
    const asked = serve({
      [GV]: [200, {}], [`${ GV }/config`]: [200, config], [`${ GV }/reports`]: [200, report()]
    });
    const w = await mountPage('Report');

    expect(asked.some((u) => u.includes('/series'))).toBe(false);
    expect(mockReplace).toHaveBeenCalledWith({ query: {} });
    w.unmount();
  });
});

describe('帳期', () => {
  it('months with translated reasons and daily seals', async() => {
    serve({
      [GV]:               [200, {}],
      [`${ GV }/config`]: [200, config],
      [`${ GV }/seals`]:  [200, {
        enabled: true,
        scope:   config.scope,
        seals:   [{
          day: '2026-09-21', total: 79.2, coveragePct: 99.6, estimatedPct: 0, gate: 'ok', intact: true
        }, {
          day: '2026-09-22', total: 70, coveragePct: 86, estimatedPct: 3, gate: 'block', intact: false
        }]
      }],
      [`${ GV }/periods`]: [200, {
        months: [{
          month: '2026-09', days: ['2026-09-21', '2026-09-22'], total: 149.2, idle: 1, unallocated: 1, estimated: 2, estimatedPct: 1.3, depts: { mfg: 50, rd: 90 }, worstGate: 'block', tampered: ['2026-09-22'], minCoverage: 86, gapMinutes: 200, sealedDays: 2, expectedDays: 30, monthDays: 30, missingDays: ['2026-09-01', '2026-09-02', '2026-09-03'], complete: false, verdict: 'block', reason: { code: 'tampered', args: { days: ['2026-09-22'] } }
        }]
      }],
    });
    const w = await mountPage('Periods');
    const html = w.html();

    expect(html).toContain('costReport.periods.reason.tampered{"days":"2026-09-22"}');
    expect(html).toContain('costReport.periods.range{\\"from\\":\\"09-01\\",\\"to\\":\\"09-03\\",\\"days\\":3}');
    expect(html).toContain('2026-09-22');
    expect(html).toContain('costReport.periods.tampered');
    expect(html).toContain('研發部');
    w.unmount();
  });
});
