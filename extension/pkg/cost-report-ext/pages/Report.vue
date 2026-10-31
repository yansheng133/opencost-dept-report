<script lang="ts">
// 部門成本: one period's report from the chosen cluster's backend. The
// period is a window (1h, 24h, 7d…) or one day (?day=YYYY-MM-DD, also
// reached from a sealed day on 帳期). The page asks again every minute,
// keeping what is being looked at.
import {
  defineComponent, ref, computed, watch, onMounted, onBeforeUnmount
} from 'vue';
import { useStore } from 'vuex';
import { useRoute, useRouter } from 'vue-router';
import { useText } from '../composables/useText';
import { useBackend } from '../composables/useBackend';
import { provideReportContext } from '../composables/useReportContext';
import TargetBar from '../components/TargetBar.vue';
import Help from '../components/Help.vue';
import { foldAll } from '../components/CrSection.vue';
import Gate from '../components/sections/Gate.vue';
import Departments from '../components/sections/Departments.vue';
import Usage from '../components/sections/Usage.vue';
import Environments from '../components/sections/Environments.vue';
import Workloads from '../components/sections/Workloads.vue';
import StorageSection from '../components/sections/StorageSection.vue';
import Unallocated from '../components/sections/Unallocated.vue';
import Trust from '../components/sections/Trust.vue';
import Actions from '../components/sections/Actions.vue';
import Rates from '../components/sections/Rates.vue';
import { getReport, ApiError, validDay } from '../utils/api';
import { visibleParts, validFilter } from '../utils/scope';
import { gapText } from '../utils/describe';
import { fmtLocal, monthFactor, n4, share } from '../utils/format';
import { COVERAGE_WARN_PCT, REFRESH_INTERVAL_MS, THIN_SAMPLE_FACTOR } from '../config/backend';
import type { Report } from '../utils/types';

export default defineComponent({
  name: 'CostReportPage',

  components: {
    TargetBar, Help, Gate, Departments, Usage, Environments, Workloads, StorageSection, Unallocated, Trust, Actions, Rates
  },

  setup() {
    const store = useStore();
    const route = useRoute();
    const router = useRouter();
    const { t } = useText(store);
    const backend = useBackend();
    const config = computed(() => backend.selected.value?.config || null);
    const ctx = provideReportContext(t, config);
    const report = ref<Report | null>(null);
    const error = ref<ApiError | null>(null);
    const loading = ref(false);
    const filter = ref('all');
    const idleShared = ref(false);
    let seq = 0;
    let timer: ReturnType<typeof setInterval> | null = null;

    const view = computed(() => {
      const day = validDay(route.query.day);
      const w = typeof route.query.window === 'string' ? route.query.window : null;

      return { day, window: day ? null : w };
    });
    const clusterId = computed(() => (backend.selected.value?.access === 'ready' ? backend.selected.value.clusterId : null));

    async function load() {
      const c = clusterId.value;

      if (!c) {
        report.value = null;

        return;
      }
      const mine = ++seq;

      loading.value = true;
      try {
        const r = await getReport(c, view.value);

        // A slow answer must not overwrite a later request (fast switching
        // between days would show one day's figures under another's date).
        if (mine === seq) {
          report.value = r;
          error.value = null;
          filter.value = validFilter(filter.value, r);
        }
      } catch (e) {
        if (mine === seq) {
          error.value = e instanceof ApiError ? e : new ApiError(0, 'invalid', String(e));
          // Another period failing must not leave the previous period's
          // figures under the new period's controls.
          report.value = null;
        }
      } finally {
        if (mine === seq) {
          loading.value = false;
        }
      }
    }

    function setQuery(q: { window?: string | null; day?: string | null }) {
      const query: Record<string, string> = {};

      if (q.day) {
        query.day = q.day;
      } else if (q.window) {
        query.window = q.window;
      }
      // The step choice is kept only for the same kind of period.
      if (typeof route.query.step === 'string') {
        query.step = route.query.step;
      }
      router.replace({ query }).catch(() => undefined);
    }

    // Also when the cluster is first known, or chosen again.
    watch([clusterId, () => view.value.day, () => view.value.window], () => load(), { immediate: true });

    onMounted(() => {
      backend.ensure();
      // Not while the tab is hidden: nobody is looking.
      timer = setInterval(() => {
        if (!document.hidden) {
          load();
        }
      }, REFRESH_INTERVAL_MS);
    });
    onBeforeUnmount(() => timer && clearInterval(timer));

    const month = computed(() => monthFactor(report.value?.window?.minutes));
    const thin = computed(() => month.value > THIN_SAMPLE_FACTOR);
    const parts = computed(() => (report.value ? visibleParts(report.value) : null));
    const windows = computed(() => report.value?.windows || config.value?.windows || []);
    const currentWindow = computed(() => (view.value.day ? null : (view.value.window || report.value?.window?.query || config.value?.defaultWindow || null)));
    const scopeText = computed(() => {
      const s = report.value?.scope || config.value?.scope;

      if (!s) {
        return '';
      }

      return s.all ? t('costReport.scope.all') : t('costReport.scope.some', { names: s.departments.map((id) => ctx.value.depts.name(id)).join(', ') });
    });
    const coverage = computed(() => {
      const c = report.value?.coverage;

      if (!c) {
        return null;
      }
      if (c.available === false) {
        return {
          title: t('costReport.coverage.unknownTitle'), text: t('costReport.coverage.unknown', { reason: c.reason || '' }), gaps: ''
        };
      }
      if ((c.pct ?? 100) < COVERAGE_WARN_PCT) {
        return {
          title: t('costReport.coverage.lowTitle', { pct: c.pct }),
          text:  t('costReport.coverage.low', { minutes: c.gapMinutes || 0 }) + (c.measuredStep ? '' : ` ${ t('costReport.coverage.guessedStep') }`),
          gaps:  (c.gaps || []).length ? t('costReport.coverage.gaps', { gaps: (c.gaps || []).map((g) => gapText(t, g)).join('; ') }) : '',
        };
      }

      return null;
    });
    const totals = computed(() => {
      const r = report.value;

      if (!r) {
        return null;
      }
      const rows = r.depts.filter((d) => filter.value === 'all' || d.id === filter.value);
      const deptSum = rows.reduce((s, d) => s + (idleShared.value ? d.shared : d.total), 0);
      const unalloc = r.unallocated ? (idleShared.value ? r.unallocated.shared : r.unallocated.separate) : null;

      return {
        deptSum, unalloc, deptLabel: filter.value === 'all' ? t('costReport.totals.depts', { count: r.depts.length }) : t('costReport.totals.dept', { name: ctx.value.depts.name(filter.value) })
      };
    });
    const freshness = computed(() => {
      const r = report.value;

      if (loading.value && !r) {
        return view.value.day ? t('costReport.report.loadingDay', { day: view.value.day }) : t('costReport.common.loading');
      }
      if (!r) {
        return '';
      }

      return r.stale ? t('costReport.report.stale', { age: r.ageSeconds }) : t('costReport.report.fresh', { age: r.ageSeconds, ttl: r.ttlSeconds });
    });
    const errorText = computed(() => {
      const e = error.value;

      if (!e) {
        return '';
      }

      return e.code === 'series_budget' ? t('costReport.errors.series_budget', { seconds: e.retryAfterSeconds ?? '?' }) : t(`costReport.errors.${ e.code }`);
    });
    const toc = computed(() => {
      const p = parts.value;
      const list: [string, string][] = [['gate', 'costReport.gate.title'], ['dept', 'costReport.dept.title'], ['usage', 'costReport.usage.title'], ['env', 'costReport.env.title'], ['workloads', 'costReport.workloads.title'], ['storage', 'costReport.storage.title']];

      if (p?.unallocRows) {
        list.push(['unalloc', 'costReport.unalloc.title']);
      }
      list.push(['trust', 'costReport.trust.title'], ['next', 'costReport.actions.title'], ['rates', 'costReport.rates.title']);

      return list.map(([id, key]) => ({ id, text: t(key) }));
    });

    return {
      t,
      ctx,
      report,
      error,
      errorText,
      loading,
      filter,
      idleShared,
      view,
      clusterId,
      windows,
      currentWindow,
      month,
      thin,
      parts,
      scopeText,
      coverage,
      totals,
      freshness,
      toc,
      fmtLocal,
      n4,
      share,
      pickWindow:  (w: string) => setQuery({ window: w }),
      pickDay:     (e: Event) => {
        const d = validDay((e.target as HTMLInputElement).value);

        if (d) {
          setQuery({ day: d });
        }
      },
      backToLive:  () => setQuery({ window: config.value?.defaultWindow || null }),
      foldAllOn:   (on: boolean) => {
        foldAll.value = { on, at: Date.now() };
      },
      noDepartment: computed(() => error.value?.code === 'no_department'),
    };
  },
});
</script>

<template>
  <div class="cr-page">
    <header class="cr-header">
      <div>
        <div
          class="text-muted cr-small"
          :class="{ 'cr-accent': view.day }"
        >
          {{ view.day ? t('costReport.report.subtitleDay', { day: view.day, label: ctx.label }) : t('costReport.report.subtitle', { label: ctx.label }) }}
        </div>
        <h1>{{ t('costReport.report.title') }}</h1>
        <div
          v-if="report"
          class="cr-meta"
        >
          <span>{{ t('costReport.report.period') }} <b class="cr-num">{{ fmtLocal(report.window.start, true) }} → {{ fmtLocal(report.window.end, true) }}</b></span>
          <span>{{ t('costReport.report.cluster') }} <b>{{ report.cluster || '—' }}</b></span>
          <span>{{ t('costReport.report.freshness') }} <b :class="{ 'cr-warn': report.stale }">{{ freshness }}</b></span>
        </div>
        <div
          v-else-if="freshness"
          class="cr-meta"
        >
          {{ freshness }}
        </div>
      </div>
      <TargetBar />
    </header>

    <template v-if="clusterId">
      <p
        v-if="scopeText"
        class="cr-scope"
      >
        {{ scopeText }}
      </p>

      <div
        v-if="noDepartment"
        class="cr-banner cr-banner-info"
      >
        {{ t('costReport.target.noDepartment') }}
      </div>
      <div
        v-else-if="error"
        class="cr-banner cr-banner-error"
      >
        <b>{{ t('costReport.report.loadFailed') }}</b>: {{ errorText }}
        <span class="text-muted cr-small">({{ error.message }})</span>
        <div>{{ t('costReport.report.loadFailedHint') }}</div>
      </div>

      <div class="cr-controls cr-sticky">
        <fieldset>
          <legend>{{ t('costReport.dept.window') }}</legend>
          <div class="cr-seg">
            <label
              v-for="w in windows"
              :key="w"
            ><input
              type="radio"
              name="cr-window"
              :checked="w === currentWindow"
              @change="pickWindow(w)"
            > {{ w }}</label>
          </div>
        </fieldset>
        <fieldset>
          <legend>{{ t('costReport.dept.day') }}</legend>
          <input
            type="date"
            class="cr-date"
            :value="view.day || ''"
            @change="pickDay"
          >
          <button
            v-if="view.day"
            type="button"
            class="btn btn-sm role-secondary ml-5"
            @click="backToLive"
          >
            {{ t('costReport.dept.backToLive') }}
          </button>
        </fieldset>
        <fieldset v-if="parts && parts.idle">
          <legend>{{ t('costReport.dept.idleMode') }}</legend>
          <div class="cr-seg">
            <label><input
              v-model="idleShared"
              type="radio"
              :value="false"
            > {{ t('costReport.dept.idleSeparate') }}</label>
            <label><input
              v-model="idleShared"
              type="radio"
              :value="true"
            > {{ t('costReport.dept.idleShared') }}</label>
          </div>
        </fieldset>
        <fieldset v-if="report && report.depts.length > 1">
          <legend>{{ t('costReport.dept.filter') }}</legend>
          <div class="cr-seg">
            <label><input
              v-model="filter"
              type="radio"
              value="all"
            > {{ t('costReport.common.all') }}</label>
            <label
              v-for="d in report.depts"
              :key="d.id"
            ><input
              v-model="filter"
              type="radio"
              :value="d.id"
            > {{ ctx.depts.name(d.id) }}</label>
          </div>
        </fieldset>
        <span
          v-if="loading && report"
          class="text-muted cr-small"
        >{{ t('costReport.common.loading') }}</span>
      </div>

      <template v-if="report">
        <div
          v-if="coverage"
          class="cr-banner cr-banner-warning"
        >
          <b>{{ coverage.title }}</b> {{ coverage.text }}
          <div
            v-if="coverage.gaps"
            class="cr-small"
          >
            {{ coverage.gaps }}
          </div>
        </div>

        <nav
          class="cr-toc"
          :aria-label="t('costReport.report.toc')"
        >
          <a
            v-for="s in toc"
            :key="s.id"
            :href="`#${ s.id }`"
          >{{ s.text }}</a>
          <span class="cr-toc-tools">
            <button
              type="button"
              class="btn btn-sm role-link"
              @click="foldAllOn(true)"
            >{{ t('costReport.report.collapseAll') }}</button>
            <button
              type="button"
              class="btn btn-sm role-link"
              @click="foldAllOn(false)"
            >{{ t('costReport.report.expandAll') }}</button>
          </span>
        </nav>

        <Gate :gate="report.gate" />

        <div
          v-if="totals"
          class="cr-totals"
        >
          <div v-if="parts && parts.idle">
            <Help
              class="cr-k"
              :text="t('costReport.totals.total')"
              :tip="t('costReport.help.total')"
            />
            <span class="cr-v cr-num">{{ n4(report.total) }}</span>
            <span class="cr-s">{{ t('costReport.totals.totalSub') }}</span>
          </div>
          <div v-else>
            <span class="cr-k">{{ t('costReport.totals.totalScoped') }}</span>
            <span class="cr-v cr-num">{{ n4(report.total) }}</span>
            <span class="cr-s">{{ t('costReport.totals.totalScopedSub') }}</span>
          </div>
          <div>
            <Help
              class="cr-k"
              :text="totals.deptLabel"
              :tip="t('costReport.help.deptTotal')"
            />
            <span class="cr-v cr-num">{{ n4(totals.deptSum) }}</span>
            <span class="cr-s">{{ t('costReport.totals.deptSub', { pct: share(totals.deptSum, report.total) }) }}</span>
          </div>
          <div v-if="parts && parts.idle">
            <Help
              class="cr-k"
              :text="t('costReport.totals.idle')"
              :tip="t('costReport.help.idleTotal')"
            />
            <span class="cr-v cr-num">{{ idleShared ? t('costReport.totals.idleShared') : n4(report.idle) }}</span>
            <span class="cr-s">{{ idleShared ? t('costReport.totals.idleSharedSub') : t('costReport.totals.ofTotal', { pct: share(report.idle, report.total) }) }}</span>
          </div>
          <div v-if="parts && parts.unallocated">
            <Help
              class="cr-k"
              :text="t('costReport.totals.unallocated')"
              :tip="t('costReport.help.unallocated')"
            />
            <span class="cr-v cr-num">{{ n4(totals.unalloc) }}</span>
            <span class="cr-s">{{ t('costReport.totals.ofTotal', { pct: share(totals.unalloc, report.total) }) }}</span>
          </div>
        </div>
        <p class="text-muted cr-small">
          {{ t('costReport.report.unit', { currency: ctx.currency }) }} ·
          {{ t('costReport.report.sample', { minutes: report.window.minutes, factor: month.toFixed(1) }) }}
          <b
            v-if="thin"
            class="cr-warn"
          >{{ t('costReport.report.thin', { minutes: report.window.minutes, factor: month.toFixed(0) }) }}</b>
        </p>

        <Departments
          :report="report"
          :filter="filter"
          :idle-shared="idleShared"
          :month="month"
          :thin="thin"
        />
        <Usage
          :report="report"
          :view="view"
          :cluster-id="clusterId"
          :filter="filter"
        />
        <Environments
          :report="report"
          :filter="filter"
        />
        <Workloads
          :report="report"
          :filter="filter"
        />
        <StorageSection
          :report="report"
          :filter="filter"
        />
        <Unallocated
          v-if="parts && parts.unallocRows"
          :report="report"
        />
        <Trust :report="report" />
        <Actions
          :report="report"
          :idle-shared="idleShared"
          :month="month"
        />
        <Rates :report="report" />
      </template>
    </template>
  </div>
</template>

<style lang="scss">
@import '../styles/report.scss';
</style>
