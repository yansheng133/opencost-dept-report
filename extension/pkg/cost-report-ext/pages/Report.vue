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
import AgeText from '../components/AgeText.vue';
import Help from '../components/Help.vue';
import { foldAll, activeFilter, openSection, foldChanged } from '../components/CrSection.vue';
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
import { getReport, ApiError, validDay, errorText as apiErrorText } from '../utils/api';
import { visibleParts, validFilter } from '../utils/scope';
import { gapText, missingMinutes, isEmptyPeriod } from '../utils/describe';
import {
  fmtLocal, monthFactor, pctValue, share, timeZone
} from '../utils/format';
import {
  checkDay, dayField, dayLocalRange, isOngoingDay, DayProblem
} from '../utils/day';
import {
  SPY_FRACTION, SPY_MIN_PX, COVERAGE_WARN_PCT, REFRESH_INTERVAL_MS, THIN_SAMPLE_FACTOR
} from '../config/backend';
import type { Report } from '../utils/types';
import { fmtMoney, moneyDigits, setMoneyBasis } from '../utils/money';

export default defineComponent({
  name: 'CostReportPage',

  components: {
    TargetBar, AgeText, Help, Gate, Departments, Usage, Environments, Workloads, StorageSection, Unallocated, Trust, Actions, Rates
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
          // One precision for every amount on the page, from the report's
          // total; the department filter does not change it.
          setMoneyBasis(r.total);
          report.value = r;
          loadedView.value = asked;
          todayUtc.value = new Date().toISOString().slice(0, 10);
          error.value = null;
          filter.value = validFilter(filter.value, r, (config.value?.departments || []).map((d) => d.id));
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
    // Today (UTC), for the date field's upper bound; updated on each load,
    // not every second (the page must not re-render while someone types).
    const todayUtc = ref(new Date().toISOString().slice(0, 10));

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
          title: t('costReport.coverage.lowTitle', { pct: pctValue(c.pct) }),
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
    // What the header says about the data's age: a key and its arguments;
    // AgeText adds the ticking age from generatedAt.
    const freshness = computed<{ key: string; args: Record<string, unknown>; born: string | null; age: number } | null>(() => {
      const r = report.value;

      if (loading.value) {
        // Also while the previous period is still on screen: what is shown is
        // about to be replaced, and must not be read or copied as the new one.
        return view.value.day ? {
          key: 'costReport.report.loadingDay', args: { day: view.value.day }, born: null, age: 0
        } : {
          key: 'costReport.report.loadingWindow', args: { window: view.value.window || config.value?.defaultWindow || '' }, born: null, age: 0
        };
      }
      if (!r) {
        return null;
      }
      const day = loadedView.value.day;
      const base = { born: r.generatedAt || null, age: r.ageSeconds };

      if (day) {
        // Today's UTC day is still running: its figures change and the
        // page keeps refreshing it (every minute, like the live windows).
        return isOngoingDay(day) ? {
          ...base, key: 'costReport.report.today', args: {}
        } : {
          ...base, key: 'costReport.report.pastDay', args: { time: fmtLocal(r.generatedAt, true) }
        };
      }
      // The page asks every minute; the server's own copy may be up to its
      // ttl older, so the data shown can be that much older again.
      const max = Math.ceil((REFRESH_INTERVAL_MS / 1000 + (r.ttlSeconds || 0)) / 60);

      return r.stale ? {
        ...base, key: 'costReport.report.stale', args: {}
      } : {
        ...base, key: 'costReport.report.fresh', args: { max }
      };
    });
    const errorText = computed(() => {
      const e = error.value;

      if (!e) {
        return '';
      }

      return apiErrorText(t, e);
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

        const running = isOngoingDay(day);
        const lag = running ? config.value?.ingestLagMinutes : 0;
        // A running day is computed up to the requested end (now minus the
        // ingest lag): say up to when.
        const upTo = running && r?.window.requested?.end ? t('costReport.report.upTo', { time: fmtLocal(r.window.requested.end) }) : '';

        return t('costReport.report.dayPeriod', {
          day, from: l.from, to: l.to, zone: timeZone() || t('costReport.report.localTime')
        }) + upTo + (lag ? t('costReport.report.ingestLag', { minutes: lag }) : '');
      }
      if (!r) {
        return '';
      }
      // The range actually asked ("last 24 hours: 10-30 12:38 → 10-31
      // 12:38"), and where the data starts if that is later.
      const req = r.window.requested;
      const range = req ? t('costReport.report.windowPeriod', {
        window: r.window.query, from: fmtLocal(req.start, true), to: fmtLocal(req.end, true)
      }) : `${ fmtLocal(r.window.start, true) } → ${ fmtLocal(r.window.end, true) }`;
      const late = req && r.window.start && Date.parse(r.window.start) - Date.parse(req.start) > 60 * 1000;
      const lag = config.value?.ingestLagMinutes;
      // The newest minutes OpenCost has not written yet are left out.
      const lagText = lag ? t('costReport.report.ingestLag', { minutes: lag }) : '';

      return `${ range }${ lagText }${ late ? t('costReport.report.dataFrom', { time: fmtLocal(r.window.start, true) }) : '' }`;
    });

    // The quick link of the section being read is highlighted: the last
    // section whose top is above the detection line (see spy); at the top of
    // the page the first section, at the bottom the last. Computed
    // from positions on every scroll (any scrolling container: Rancher
    // scrolls a div, not the window) and after each reload.
    const current = ref('');
    const page = ref<HTMLElement | null>(null);
    const bar = ref<HTMLElement | null>(null);
    let frame = 0;
    let scroller: HTMLElement | null = null;
    // After a quick link is used, its section stays highlighted until the
    // reader scrolls (the section may not reach the line at the page end).
    let pinned = '';
    let pinnedAt = 0;

    function atBottom(): boolean {
      const el = scroller || (document.scrollingElement as HTMLElement | null);

      return !!el && el.scrollHeight > el.clientHeight && el.scrollTop + el.clientHeight >= el.scrollHeight - 2;
    }

    function spy() {
      frame = 0;
      const ids = toc.value.map((x) => x.id);

      if (pinned && Date.now() - pinnedAt < 1000) {
        current.value = pinned;

        return;
      }
      pinned = '';
      // The detection line is a third of the way down the visible area
      // below the sticky bar: a section whose title is in the upper third
      // is the one being read.
      const top = bar.value?.getBoundingClientRect().bottom || 0;
      const bottom = scroller ? Math.min(scroller.getBoundingClientRect().bottom, window.innerHeight) : window.innerHeight;
      const line = top + Math.max(SPY_MIN_PX, (bottom - top) * SPY_FRACTION);
      const last = ids.length ? document.getElementById(ids[ids.length - 1]) : null;
      const el = scroller || (document.scrollingElement as HTMLElement | null);

      // Scrolled to the end: the last section is current even if it is too
      // short to reach the line, but only when the page was actually
      // scrolled and that section is on screen (after "collapse all" the
      // whole page fits and the top section stays current).
      if (last && atBottom() && (el?.scrollTop || 0) > 0 && last.getBoundingClientRect().top < window.innerHeight) {
        current.value = ids[ids.length - 1];

        return;
      }
      let cur = ids[0] || '';

      ids.forEach((id) => {
        const el = document.getElementById(id);

        if (el && el.getBoundingClientRect().top <= line) {
          cur = id;
        }
      });
      current.value = cur;
    }
    const onScroll = (e: Event) => {
      if (e.target instanceof HTMLElement) {
        scroller = e.target;
      }
      if (!frame) {
        frame = requestAnimationFrame(spy);
      }
    };
    const spyLater = async() => {
      await nextTick();
      spy();
    };

    // A quick link opens a folded section first, then scrolls to it.
    // The highlight follows only once the scroll has landed: sections above
    // may still be growing (charts, tables) when it starts, so it is
    // aligned again after two frames.
    async function goTo(id: string) {
      openSection(id);
      await nextTick();
      const frameDone = () => new Promise((resolve) => requestAnimationFrame(() => resolve(null)));

      document.getElementById(id)?.scrollIntoView({ block: 'start' });
      await frameDone();
      await frameDone();
      document.getElementById(id)?.scrollIntoView({ block: 'start' });
      pinned = id;
      pinnedAt = Date.now();
      current.value = id;
    }

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
    // Also after any section folds or unfolds: everything below it moved.
    watch([toc, empty, report, foldChanged], spyLater);
    // Arriving with #section in the address: open it and go there once the
    // first report is shown.
    const stopHash = watch(report, async(r) => {
      const id = (window.location.hash || '').slice(1);

      if (r && id && toc.value.some((x) => x.id === id)) {
        stopHash();
        await nextTick();
        goTo(id);
      } else if (r) {
        stopHash();
      }
    });

    onBeforeUnmount(() => {
      document.removeEventListener('scroll', onScroll, { capture: true });
      resize?.disconnect();
      if (frame) {
        cancelAnimationFrame(frame);
      }
    });

    // The date field: see dayField (a picker choice applies at once;
    // typed dates only on Enter or leaving the field). Days before the
    // earliest data or after today (UTC) are refused with a reason.
    const dayProblem = ref<DayProblem>(null);

    // A date that cannot be used: the field goes back to the period being
    // shown and a hint stays next to it (aria-invalid) until a valid date
    // is applied, the field is emptied or the page goes back to live.
    function clearProblem() {
      dayProblem.value = null;
    }

    // The hint describes a rejected value; once the reader moves on (another
    // window or day, the filter, idle mode) it no longer applies.
    watch([() => view.value.window, () => view.value.day, filter, idleShared], clearProblem);
    function showProblem(p: DayProblem) {
      dayProblem.value = p;
      if (p) {
        dayDraft.value = view.value.day || '';
      }
    }
    // What the field shows. While it has focus it keeps what is being
    // typed; any re-render (data refresh, a ticking clock) must not reset it
    // to the period's day. It follows the period again on commit and blur.
    const dayDraft = ref(view.value.day || '');
    const dayFocused = ref(false);

    watch(() => view.value.day, (d) => {
      if (!dayFocused.value) {
        dayDraft.value = d || '';
      }
    });
    const dayMin = computed(() => (config.value?.dataSince ? config.value.dataSince.slice(0, 10) : null));
    const dayMax = computed(() => todayUtc.value);
    const field = dayField({
      commit: (d) => {
        clearProblem();
        // The field shows what is now being applied, not the previous day
        // until the address changes.
        dayDraft.value = d;
        if (d !== view.value.day) {
          setQuery({ day: d });
        }
      },
      onProblem: (p) => showProblem(p),
      range:     () => ({ min: dayMin.value, max: dayMax.value }),
    });

    return {
      t,
      ctx,
      empty,
      periodText,
      current,
      page,
      bar,
      fmtMoney,
      moneyDigits,
      loadedView,
      goTo,
      dayProblem,
      config,
      zoneName:   timeZone() || t('costReport.report.localTime'),
      dayMin,
      dayMax,
      dayDraft,
      dayKey:     () => field.keydown(),
      dayFocus:   () => {
        dayFocused.value = true;
      },
      dayInput:   (e: Event) => {
        dayDraft.value = (e.target as HTMLInputElement).value;
        // Typing a usable date again clears the hint about the rejected one.
        if (dayProblem.value && !checkDay(dayDraft.value, dayMin.value, dayMax.value)) {
          clearProblem();
        }
      },
      dayChange:  (e: Event) => {
        dayDraft.value = (e.target as HTMLInputElement).value;
        field.change(dayDraft.value);
      },
      dayCommit:  (e: Event) => field.flush((e.target as HTMLInputElement).value),
      dayBlur:    (e: Event) => {
        dayFocused.value = false;
        // Leaving the field (Tab, a click elsewhere) applies a valid date at
        // once, so the field never shows a day the page is not using. The
        // browser reports an empty value for a half-typed date; the last
        // complete value typed is used then.
        const v = (e.target as HTMLInputElement).value || dayDraft.value;

        if (v && v !== view.value.day) {
          field.flush(v);
        } else if (!v) {
          clearProblem();
          dayDraft.value = view.value.day || '';
        }
      },
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
      pickWindow:  (w: string) => {
        clearProblem();
        setQuery({ window: w });
      },
      backToLive:  () => {
        clearProblem();
        setQuery({ window: config.value?.defaultWindow || null });
      },
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
          :class="{ 'cr-accent': loadedView.day }"
        >
          {{ loadedView.day ? t('costReport.report.subtitleDay', { day: loadedView.day, label: ctx.label }) : t('costReport.report.subtitle', { label: ctx.label }) }}
        </div>
        <h1>{{ t('costReport.report.title') }}</h1>
        <div
          v-if="report"
          class="cr-meta"
        >
          <span>{{ t('costReport.report.period') }} <b class="cr-num">{{ periodText }}</b></span>
          <span>{{ t('costReport.report.cluster') }} <b>{{ report.cluster || '—' }}</b></span>
          <span v-if="freshness">{{ t('costReport.report.freshness') }} <b :class="{ 'cr-warn': report.stale && !loading }"><AgeText
            :text-key="freshness.key"
            :args="freshness.args"
            :born="freshness.born"
            :fallback-age="freshness.age"
          /></b></span>
        </div>
        <div
          v-else-if="freshness"
          class="cr-meta"
        >
          <AgeText
            :text-key="freshness.key"
            :args="freshness.args"
            :born="freshness.born"
            :fallback-age="freshness.age"
          />
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
                :value="w"
                :checked="w === currentWindow"
                @change="pickWindow(w)"
              > {{ w }}</label>
            </div>
          </fieldset>
          <fieldset class="cr-day-field">
            <legend>{{ t('costReport.dept.day') }}</legend>
            <div class="cr-day">
              <input
                type="date"
                class="cr-date"
                :aria-invalid="dayProblem ? 'true' : undefined"
                aria-describedby="cr-day-hint"
                :value="dayDraft"
                :min="dayMin || undefined"
                :max="dayMax"
                @keydown.enter.prevent="dayCommit"
                @keydown="dayKey"
                @focus="dayFocus"
                @input="dayInput"
                @change="dayChange"
                @blur="dayBlur"
              >
              <button
                v-if="view.day"
                type="button"
                class="btn btn-sm role-link cr-back"
                @click="backToLive"
              >
                {{ t('costReport.dept.backToLive') }}
              </button>
              <span
                v-if="dayProblem"
                id="cr-day-hint"
                class="cr-day-hint"
                role="alert"
              >{{ t(`costReport.dept.dayProblem.${ dayProblem === 'before' && dayMin === dayMax ? 'only' : dayProblem }`, { min: dayMin || '—', max: dayMax }) }}</span>
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
            @click.prevent="goTo(s.id)"
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
          <p v-if="config && config.dataSince">
            {{ t('costReport.empty.dataSince', { time: fmtLocal(config.dataSince, true), zone: zoneName }) }}
          </p>
          <ul>
            <li>{{ t('costReport.empty.before') }}</li>
            <li>{{ t('costReport.empty.outage') }}</li>
            <li v-if="!loadedView.day">
              {{ t('costReport.empty.labels', { label: ctx.label }) }}
            </li>
          </ul>
        </div>

        <template v-else>
          <div
            v-if="coverage"
            class="cr-banner cr-banner-warning"
          >
            <b>{{ coverage.title }}</b> {{ coverage.text }}
            <a
              href="#trust"
              @click.prevent="goTo('trust')"
            >{{ t('costReport.coverage.more') }}</a>
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
              <span class="cr-v cr-num">{{ fmtMoney(report.total) }} <small class="cr-cur">{{ ctx.currency }}</small></span>
              <span class="cr-s">{{ t('costReport.totals.totalSub') }}</span>
            </div>
            <div v-else>
              <span class="cr-k">{{ t('costReport.totals.totalScoped') }}</span>
              <span class="cr-v cr-num">{{ fmtMoney(report.total) }} <small class="cr-cur">{{ ctx.currency }}</small></span>
              <span class="cr-s">{{ t('costReport.totals.totalScopedSub') }}</span>
            </div>
            <div>
              <Help
                class="cr-k"
                :text="totals.deptLabel"
                :tip="t('costReport.help.deptTotal')"
              />
              <span class="cr-v cr-num">{{ fmtMoney(totals.deptSum) }} <small class="cr-cur">{{ ctx.currency }}</small></span>
              <span class="cr-s">{{ t('costReport.totals.deptSub', { pct: share(totals.deptSum, report.total) }) }}</span>
            </div>
            <div v-if="parts && parts.idle">
              <Help
                class="cr-k"
                :text="t('costReport.totals.idle')"
                :tip="t('costReport.help.idleTotal')"
              />
              <span class="cr-v cr-num">{{ idleShared ? t('costReport.totals.idleShared') : fmtMoney(report.idle) }} <small
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
              <span class="cr-v cr-num">{{ fmtMoney(totals.unalloc) }} <small class="cr-cur">{{ ctx.currency }}</small></span>
              <span class="cr-s">{{ t('costReport.totals.ofTotal', { pct: share(totals.unalloc, report.total) }) }}</span>
            </div>
          </div>
          <p class="text-muted cr-small">
            {{ t('costReport.report.unit', { currency: ctx.currency }) }} ·
            <template v-if="moneyDigits === 4">
              {{ t('costReport.report.precision4') }} ·
            </template>
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
      <div
        v-else-if="loading"
        class="cr-empty cr-placeholder"
        aria-busy="true"
      >
        <i class="icon icon-spinner icon-spin" />
        <AgeText
          v-if="freshness"
          :text-key="freshness.key"
          :args="freshness.args"
          :born="freshness.born"
          :fallback-age="freshness.age"
        />
      </div>
    </template>
  </div>
</template>

<style lang="scss">
@import '../styles/report.scss';
</style>
