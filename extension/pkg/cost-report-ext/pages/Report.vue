<script lang="ts">
// 部門成本: one period's report from the chosen cluster's backend. The
// period is a window (1h, 24h, 7d…) or one day (?day=YYYY-MM-DD, also
// reached from a sealed day on 帳期). The page asks again every minute,
// keeping what is being looked at.
import {
  defineComponent, ref, computed, watch, nextTick, onMounted, onBeforeUnmount
} from 'vue';
import { useStore } from 'vuex';
import { useRoute, useRouter } from 'vue-router';
import { useText } from '../composables/useText';
import { useBackend } from '../composables/useBackend';
import { provideReportContext } from '../composables/useReportContext';
import TargetBar from '../components/TargetBar.vue';
import Help from '../components/Help.vue';
import { foldAll, activeFilter } from '../components/CrSection.vue';
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
import { gapText, missingMinutes, isEmptyPeriod } from '../utils/describe';
import {
  fmtLocal, monthFactor, moneyCol, share, timeZone
} from '../utils/format';
import { completeDay, dayLocalRange, isOngoingDay } from '../utils/day';
import { DAY_SETTLE_MS, COVERAGE_WARN_PCT, REFRESH_INTERVAL_MS, THIN_SAMPLE_FACTOR } from '../config/backend';
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
    // The period the shown report belongs to. While another period loads,
    // the (dimmed) content keeps describing what it is, not what was asked.
    const loadedView = ref<{ day: string | null; window: string | null }>({ day: null, window: null });
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
      const asked = { ...view.value };

      loading.value = true;
      try {
        const r = await getReport(c, asked);

        // A slow answer must not overwrite a later request (fast switching
        // between days would show one day's figures under another's date).
        if (mine === seq) {
          report.value = r;
          loadedView.value = asked;
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

      const base = s.all ? t('costReport.scope.all') : t('costReport.scope.some', { names: s.departments.map((id) => ctx.value.depts.name(id)).join(', ') });

      // A filter narrows only the department sections; say so, or the
      // cluster-wide sections below read as that department's.
      return filter.value === 'all' ? base : t('costReport.scope.filtered', { name: ctx.value.depts.name(filter.value) });
    });

    watch(filter, (f) => {
      activeFilter.value = f;
    }, { immediate: true });
    onBeforeUnmount(() => {
      activeFilter.value = 'all';
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
        const m = missingMinutes(c);
        const why = [
          m.edge ? t('costReport.coverage.edge', { count: m.edge }) : '',
          m.inside ? t('costReport.coverage.inside', { count: m.inside }) : '',
        ].filter(Boolean).join(t('costReport.coverage.sep'));

        return {
          title: t('costReport.coverage.lowTitle', { pct: c.pct }),
          text:  `${ why ? t('costReport.coverage.why', { why }) : '' } ${ t('costReport.coverage.low') }${ c.measuredStep ? '' : ` ${ t('costReport.coverage.guessedStep') }` }`,
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

      if (loading.value) {
        // Also while the previous period is still on screen: what is shown is
        // about to be replaced, and must not be read or copied as the new one.
        return view.value.day ? t('costReport.report.loadingDay', { day: view.value.day }) : t('costReport.report.loadingWindow', { window: view.value.window || config.value?.defaultWindow || '' });
      }
      if (!r) {
        return '';
      }
      const day = loadedView.value.day;

      if (day) {
        // Today's UTC day is still running: its figures change and the
        // page keeps refreshing it.
        return isOngoingDay(day) ? t('costReport.report.today', { age: r.ageSeconds }) : t('costReport.report.pastDay', { time: fmtLocal(r.generatedAt, true) });
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

    const empty = computed(() => isEmptyPeriod(report.value?.gate));
    const periodText = computed(() => {
      const r = report.value;

      const day = loadedView.value.day;

      if (day) {
        const l = dayLocalRange(day);

        return t('costReport.report.dayPeriod', {
          day, from: l.from, to: l.to, zone: timeZone() || t('costReport.report.localTime')
        });
      }

      return r ? `${ fmtLocal(r.window.start, true) } → ${ fmtLocal(r.window.end, true) }` : '';
    });

    // The quick link of the section being read is highlighted: the last
    // section whose top has passed under the sticky bar, or the first one
    // at the top of the page. Computed from positions on every scroll (any
    // scrolling container: Rancher scrolls a div, not the window) and after
    // each reload, so it never keeps a stale section.
    const current = ref('');
    const page = ref<HTMLElement | null>(null);
    const bar = ref<HTMLElement | null>(null);
    let frame = 0;

    function spy() {
      frame = 0;
      const limit = (bar.value?.getBoundingClientRect().bottom || 0) + 12;
      const ids = toc.value.map((x) => x.id);
      let cur = ids[0] || '';

      ids.forEach((id) => {
        const el = document.getElementById(id);

        if (el && el.getBoundingClientRect().top <= limit) {
          cur = id;
        }
      });
      current.value = cur;
    }
    const onScroll = () => {
      if (!frame) {
        frame = requestAnimationFrame(spy);
      }
    };
    const spyLater = async() => {
      await nextTick();
      spy();
    };

    // Table headers stick under the bar: its height, as a CSS variable.
    let resize: ResizeObserver | null = null;

    onMounted(() => {
      document.addEventListener('scroll', onScroll, { capture: true, passive: true });
      if (typeof ResizeObserver !== 'undefined') {
        resize = new ResizeObserver(() => {
          page.value?.style.setProperty('--cr-sticky-h', `${ bar.value?.offsetHeight || 0 }px`);
        });
      }
      spyLater();
    });
    watch(bar, (el) => {
      if (el) {
        resize?.observe(el);
      }
    });
    watch([toc, empty, report], spyLater);
    onBeforeUnmount(() => {
      document.removeEventListener('scroll', onScroll, { capture: true });
      resize?.disconnect();
      if (frame) {
        cancelAnimationFrame(frame);
      }
    });

    // The date field reports every typed segment (0002-…, 0020-…); only a
    // complete date that stays for half a second, or is confirmed with
    // Enter or by leaving the field, changes the period.
    let dayTimer: ReturnType<typeof setTimeout> | null = null;

    function commitDay(v: string) {
      if (dayTimer) {
        clearTimeout(dayTimer);
        dayTimer = null;
      }
      const d = completeDay(v);

      if (d && d !== view.value.day) {
        setQuery({ day: d });
      }
    }

    function typeDay(e: Event) {
      const v = (e.target as HTMLInputElement).value;

      if (dayTimer) {
        clearTimeout(dayTimer);
      }
      dayTimer = completeDay(v) ? setTimeout(() => commitDay(v), DAY_SETTLE_MS) : null;
    }
    onBeforeUnmount(() => dayTimer && clearTimeout(dayTimer));

    // One decimal count for the four cards.
    const cardMoney = computed(() => {
      const r = report.value;

      return moneyCol(r ? [r.total, totals.value?.deptSum, r.idle, totals.value?.unalloc] : []);
    });

    return {
      t,
      ctx,
      empty,
      periodText,
      current,
      page,
      bar,
      cardMoney,
      loadedView,
      typeDay,
      commitDay:  (e: Event) => commitDay((e.target as HTMLInputElement).value),
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
      share,
      pickWindow:  (w: string) => setQuery({ window: w }),
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
  <div
    ref="page"
    class="cr-page"
  >
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
          <span>{{ t('costReport.report.period') }} <b class="cr-num">{{ periodText }}</b></span>
          <span>{{ t('costReport.report.cluster') }} <b>{{ report.cluster || '—' }}</b></span>
          <span>{{ t('costReport.report.freshness') }} <b :class="{ 'cr-warn': report.stale && !loading }">{{ freshness }}</b></span>
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

      <!-- Controls and the quick links stay in view while scrolling. -->
      <div
        ref="bar"
        class="cr-sticky"
      >
        <div class="cr-controls">
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
            <div class="cr-day">
              <input
                type="date"
                class="cr-date"
                :value="view.day || ''"
                @input="typeDay"
                @change="typeDay"
                @blur="commitDay"
                @keydown.enter="commitDay"
              >
              <button
                v-if="view.day"
                type="button"
                class="btn btn-sm role-link cr-back"
                @click="backToLive"
              >
                {{ t('costReport.dept.backToLive') }}
              </button>
            </div>
          </fieldset>
          <fieldset v-if="parts && parts.idle && !empty">
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
          <!-- Always reachable, whatever the width of the quick links below. -->
          <span
            v-if="report && !empty"
            class="cr-toc-tools"
          >
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
        </div>
        <nav
          v-if="report && !empty"
          class="cr-toc"
          :aria-label="t('costReport.report.toc')"
        >
          <a
            v-for="s in toc"
            :key="s.id"
            :href="`#${ s.id }`"
            :class="{ active: current === s.id }"
          >{{ s.text }}</a>
        </nav>
      </div>

      <div
        v-if="report"
        class="cr-content"
        :class="{ 'cr-loading': loading }"
        :aria-busy="loading"
      >
        <!-- A period without any cost: one clear card, not every section full of zeros. -->
        <div
          v-if="empty"
          class="cr-empty"
        >
          <h2>{{ loadedView.day ? t('costReport.empty.titleDay', { day: loadedView.day }) : t('costReport.empty.title', { period: periodText }) }}</h2>
          <p>{{ t('costReport.empty.text') }}</p>
          <ul>
            <li>{{ t('costReport.empty.before') }}</li>
            <li>{{ t('costReport.empty.outage') }}</li>
            <li v-if="!loadedView.day">
              {{ t('costReport.empty.labels', { label: ctx.label }) }}
            </li>
          </ul>
          <button
            v-if="loadedView.day"
            type="button"
            class="btn role-primary"
            @click="backToLive"
          >
            {{ t('costReport.dept.backToLive') }}
          </button>
        </div>

        <template v-else>
          <div
            v-if="coverage"
            class="cr-banner cr-banner-warning"
          >
            <b>{{ coverage.title }}</b> {{ coverage.text }}
            <a href="#trust">{{ t('costReport.coverage.more') }}</a>
          </div>

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
              <span class="cr-v cr-num">{{ cardMoney(report.total) }} <small class="cr-cur">{{ ctx.currency }}</small></span>
              <span class="cr-s">{{ t('costReport.totals.totalSub') }}</span>
            </div>
            <div v-else>
              <span class="cr-k">{{ t('costReport.totals.totalScoped') }}</span>
              <span class="cr-v cr-num">{{ cardMoney(report.total) }} <small class="cr-cur">{{ ctx.currency }}</small></span>
              <span class="cr-s">{{ t('costReport.totals.totalScopedSub') }}</span>
            </div>
            <div>
              <Help
                class="cr-k"
                :text="totals.deptLabel"
                :tip="t('costReport.help.deptTotal')"
              />
              <span class="cr-v cr-num">{{ cardMoney(totals.deptSum) }} <small class="cr-cur">{{ ctx.currency }}</small></span>
              <span class="cr-s">{{ t('costReport.totals.deptSub', { pct: share(totals.deptSum, report.total) }) }}</span>
            </div>
            <div v-if="parts && parts.idle">
              <Help
                class="cr-k"
                :text="t('costReport.totals.idle')"
                :tip="t('costReport.help.idleTotal')"
              />
              <span class="cr-v cr-num">{{ idleShared ? t('costReport.totals.idleShared') : cardMoney(report.idle) }} <small
                v-if="!idleShared"
                class="cr-cur"
              >{{ ctx.currency }}</small></span>
              <span class="cr-s">{{ idleShared ? t('costReport.totals.idleSharedSub') : t('costReport.totals.ofTotal', { pct: share(report.idle, report.total) }) }}</span>
            </div>
            <div v-if="parts && parts.unallocated">
              <Help
                class="cr-k"
                :text="t('costReport.totals.unallocated')"
                :tip="t('costReport.help.unallocated')"
              />
              <span class="cr-v cr-num">{{ cardMoney(totals.unalloc) }} <small class="cr-cur">{{ ctx.currency }}</small></span>
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

          <Gate :gate="report.gate" />

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
            :idle-shared="idleShared"
          />
          <Trust :report="report" />
          <Actions
            :report="report"
            :idle-shared="idleShared"
            :month="month"
          />
          <Rates
            :report="report"
            :filter="filter"
          />
        </template>
      </div>
    </template>
  </div>
</template>

<style lang="scss">
@import '../styles/report.scss';
</style>
