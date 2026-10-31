<script lang="ts">
// 各部門的使用率趨勢. The report carries the series at the default step;
// another step is asked from `series` when the user picks it. The choice
// is kept in the address (?step=) so a shared link shows the same chart,
// and kept across the page's refresh every minute.
import {
  defineComponent, PropType, computed, ref, watch
} from 'vue';
import { useRoute, useRouter } from 'vue-router';
import CrSection from '../CrSection.vue';
import { useReportContext } from '../../composables/useReportContext';
import { getSeries, ApiError, errorText as apiErrorText } from '../../utils/api';
import {
  averages, buildChart, nearestPoint, pathOf, valueOf, wantedStep, xOf, BOX, Metric
} from '../../utils/series';
import {
  fmtCores, fmtLocal, pctValue, stepParts, timeZone
} from '../../utils/format';
import type { Report, Series } from '../../utils/types';

export default defineComponent({
  name: 'CostReportUsage',

  components: { CrSection },

  props: {
    report:    { type: Object as PropType<Report>, required: true },
    view:      { type: Object as PropType<{ window: string | null; day: string | null }>, required: true },
    clusterId: { type: String, required: true },
    filter:    { type: String, required: true },
  },

  setup(props) {
    const ctx = useReportContext();
    const route = useRoute();
    const router = useRouter();
    const metric = ref<Metric>('cpu');
    const series = ref<Series | null>(props.report.series);
    const busy = ref(false);
    const pending = ref('');
    const failure = ref('');
    const hover = ref<number | null>(null);
    let seq = 0;
    const chosen = () => (typeof route.query.step === 'string' ? route.query.step : null);
    const stepText = (s: string) => {
      const p = stepParts(s);

      return p.unit ? ctx.value.t(`costReport.usage.stepUnit.${ p.unit }`, { n: p.n }) : s;
    };
    const errorText = (e: unknown) => {
      if (e instanceof ApiError) {
        // Without its final period: it goes inside a sentence that has one.
        return apiErrorText(ctx.value.t, e).replace(/[。.]\s*$/, '');
      }

      return String(e);
    };

    function setStepInRoute(step: string | null, dflt: string | undefined) {
      const query = { ...route.query };

      if (step && step !== dflt) {
        query.step = step;
      } else {
        delete query.step;
      }
      router.replace({ query }).catch(() => undefined);
    }

    async function load(step: string) {
      const base = props.report.series;

      failure.value = '';
      if (!base || step === series.value?.step) {
        return;
      }
      // The report's own series needs no request.
      if (step === base.step) {
        series.value = base;
        setStepInRoute(step, base.defaultStep);

        return;
      }
      const mine = ++seq;

      busy.value = true;
      pending.value = step;
      try {
        const s = await getSeries(props.clusterId, props.view, step);

        // A slow answer must not replace a later choice.
        if (mine === seq) {
          series.value = s;
          setStepInRoute(step, base.defaultStep);
        }
      } catch (e) {
        if (mine === seq) {
          failure.value = ctx.value.t('costReport.usage.stepFailed', {
            step: stepText(step), error: errorText(e), current: stepText(series.value?.step || base.step)
          });
        }
      } finally {
        if (mine === seq) {
          busy.value = false;
        }
      }
    }

    // A new report (refresh, other period): start from its series, then
    // ask again for the user's step if this period offers it.
    watch(() => props.report, (r) => {
      hover.value = null;
      series.value = r.series;
      const want = wantedStep(chosen(), r.series);

      if (want) {
        load(want);
      }
    }, { immediate: true });

    watch(series, () => {
      hover.value = null;
    });

    const ids = computed(() => props.report.depts.map((d) => d.id).filter((id) => props.filter === 'all' || id === props.filter));
    // A single day is labelled every 2 h; other periods pick by length.
    const chart = computed(() => (series.value ? buildChart(series.value, ids.value, metric.value, Date.now(), props.view.day ? 2 * 3600 * 1000 : undefined) : null));
    const missing = computed(() => (series.value ? series.value.expected - series.value.covered : 0));
    const tip = computed(() => {
      const c = chart.value;

      const p = c && hover.value !== null ? c.pts[hover.value] : null;

      if (!c || !p) {
        return null;
      }
      const x = xOf(p.ts, c.t0, c.t1);

      return {
        x,
        left:    `${ Math.min(Math.max(x / BOX.W * 100, 12), 88) }%`,
        p,
        partial: !!p.partial || hover.value === c.partialIdx,
        rows:    ids.value.map((id) => {
          const v = valueOf(p, id, metric.value);
          const raw = p.depts?.[id];

          return {
            id, v, y: v === null ? null : BOX.T + (BOX.H - BOX.T - BOX.B) * (1 - v / c.maxY), raw
          };
        }),
      };
    });

    function move(e: MouseEvent) {
      const c = chart.value;
      const svg = e.currentTarget as SVGSVGElement;

      if (!c) {
        return;
      }
      const r = svg.getBoundingClientRect();

      hover.value = nearestPoint(c, (e.clientX - r.left) / r.width * BOX.W);
    }

    // Folded: the period's average per department, for the chosen resource.
    const summary = computed(() => {
      const list = averages(series.value, ids.value, metric.value);

      return list.length ? ctx.value.t(`costReport.usage.summaryAvg.${ metric.value }`, { list: list.map((a) => `${ ctx.value.depts.name(a.id) } ${ pctValue(a.avg) }`).join(' · ') }) : ctx.value.t('costReport.usage.summaryNone');
    });

    return {
      summary, pctValue, fmtCores, ctx, metric, series, busy, pending, failure, chart, missing, tip, hover, move, load, stepText, pathOf, fmtLocal, BOX, zone: timeZone()
    };
  },
});
</script>

<template>
  <CrSection
    id="usage"
    :title="ctx.t('costReport.usage.title')"
    :description="ctx.t('costReport.usage.description')"
    :summary="summary"
  >
    <div class="cr-controls">
      <fieldset>
        <legend>{{ ctx.t('costReport.usage.metric') }}</legend>
        <div class="cr-seg">
          <label><input
            v-model="metric"
            type="radio"
            value="cpu"
          > {{ ctx.t('costReport.resource.cpu') }}</label>
          <label><input
            v-model="metric"
            type="radio"
            value="ram"
          > {{ ctx.t('costReport.resource.ram') }}</label>
        </div>
      </fieldset>
      <fieldset v-if="series && series.options && series.options.length > 1">
        <legend>{{ ctx.t('costReport.usage.step') }}</legend>
        <div class="cr-seg">
          <label
            v-for="o in series.options"
            :key="o"
          ><input
            type="radio"
            name="cr-step"
            :value="o"
            :checked="o === (busy ? pending : series.step)"
            :disabled="busy"
            @change="load(o)"
          > {{ stepText(o) }}</label>
        </div>
      </fieldset>
    </div>
    <p
      v-if="!series || !series.points || !series.points.length"
      class="text-muted"
    >
      {{ ctx.t('costReport.usage.none') }}
    </p>
    <p
      v-else-if="!chart"
      class="text-muted"
    >
      {{ ctx.t('costReport.usage.noTimes') }}
    </p>
    <div
      v-else
      class="cr-chart"
      :class="{ busy }"
    >
      <svg
        :viewBox="`0 0 ${ BOX.W } ${ BOX.H }`"
        preserveAspectRatio="none"
        role="img"
        :aria-label="ctx.t('costReport.usage.aria')"
        @mousemove="move"
        @mouseleave="hover = null"
      >
        <!-- Transparent, not none: SVG only receives pointer events where something is painted. -->
        <rect
          :x="BOX.L"
          :y="BOX.T"
          :width="BOX.W - BOX.L - BOX.R"
          :height="BOX.H - BOX.T - BOX.B"
          fill="transparent"
        />
        <g class="cr-grid">
          <template
            v-for="tk in chart.yTicks"
            :key="tk.v"
          >
            <line
              :x1="BOX.L"
              :x2="BOX.W - BOX.R"
              :y1="tk.y"
              :y2="tk.y"
            />
            <text
              :x="BOX.L - 8"
              :y="tk.y + 3"
              text-anchor="end"
              class="cr-axis"
            >{{ tk.v.toFixed(0) }}%</text>
          </template>
        </g>
        <text
          v-for="tk in chart.xTicks"
          :key="tk.start"
          :x="tk.x"
          :y="BOX.H - 8"
          text-anchor="middle"
          class="cr-axis"
        >{{ fmtLocal(tk.start, tk.withDate) }}</text>
        <g
          v-for="l in chart.lines"
          :key="l.id"
        >
          <template
            v-for="(seg, i) in l.segments"
            :key="i"
          >
            <circle
              v-if="seg.length === 1"
              :cx="seg[0][0]"
              :cy="seg[0][1]"
              r="3"
              :fill="ctx.depts.color(l.id)"
            />
            <path
              v-else
              :d="pathOf(seg)"
              :stroke="ctx.depts.color(l.id)"
              class="cr-path"
            />
            <template v-if="l.marks && seg.length > 1">
              <circle
                v-for="(xy, j) in seg"
                :key="j"
                :cx="xy[0]"
                :cy="xy[1]"
                r="1.6"
                :fill="ctx.depts.color(l.id)"
              />
            </template>
          </template>
          <template
            v-for="(d, k) in l.dashed"
            :key="`d${ k }`"
          >
            <path
              v-if="d.from"
              :d="pathOf([d.from, d.to])"
              :stroke="ctx.depts.color(l.id)"
              class="cr-path cr-partial"
            />
            <circle
              :cx="d.to[0]"
              :cy="d.to[1]"
              r="2.4"
              :stroke="ctx.depts.color(l.id)"
              class="cr-partial-dot"
            />
          </template>
        </g>
        <text
          v-for="e in chart.endLabels"
          :key="e.id"
          :x="BOX.W - BOX.R + 6"
          :y="e.y + 4"
          :fill="ctx.depts.color(e.id)"
          class="cr-end"
        >{{ ctx.depts.name(e.id) }}</text>
        <template v-if="tip">
          <line
            :x1="tip.x"
            :x2="tip.x"
            :y1="BOX.T"
            :y2="BOX.H - BOX.B"
            class="cr-hit"
          />
          <template
            v-for="r in tip.rows"
            :key="r.id"
          >
            <circle
              v-if="r.y !== null"
              :cx="tip.x"
              :cy="r.y"
              r="3"
              :fill="ctx.depts.color(r.id)"
            />
          </template>
        </template>
      </svg>
      <div
        v-if="tip"
        class="cr-chart-tip"
        :style="{ left: tip.left }"
      >
        <b>{{ tip.p.depts ? ctx.t('costReport.usage.tipFrom', { start: fmtLocal(tip.p.start, true), minutes: tip.p.minutes }) : fmtLocal(tip.p.start, true) }}</b>
        <div
          v-if="!tip.p.depts"
          class="cr-warn"
        >
          {{ ctx.t('costReport.usage.tipNoData') }}
        </div>
        <div
          v-if="tip.partial"
          class="text-muted cr-small"
        >
          {{ ctx.t('costReport.usage.tipPartial') }}
        </div>
        <div
          v-for="r in tip.rows"
          :key="r.id"
        >
          <span
            class="cr-swatch"
            :style="{ background: ctx.depts.color(r.id) }"
          />
          {{ ctx.depts.name(r.id) }}
          <b>{{ r.v === null ? ctx.t('costReport.usage.tipNoValue') : pctValue(r.v) }}</b>
          <span
            v-if="metric === 'cpu' && r.raw && r.raw.cpuReq"
            class="text-muted cr-small"
          >{{ ctx.t('costReport.usage.tipCpu', { req: fmtCores(r.raw.cpuReq), use: fmtCores(r.raw.cpuUse, r.raw.cpu) }) }}</span>
        </div>
      </div>
      <!-- The lines are named at their ends; a legend only when there are none. -->
      <div
        v-if="!chart.endLabels.length"
        class="cr-legend"
      >
        <span
          v-for="l in chart.lines"
          :key="l.id"
        ><span
          class="cr-swatch"
          :style="{ background: ctx.depts.color(l.id) }"
        />{{ ctx.depts.name(l.id) }}</span>
      </div>
    </div>
    <p
      v-if="busy"
      class="text-muted cr-small"
    >
      {{ ctx.t('costReport.usage.loadingStep', { step: stepText(pending) }) }}
    </p>
    <p
      v-if="failure"
      class="cr-warn cr-small"
    >
      {{ failure }}
    </p>
    <p
      v-if="chart && series"
      class="text-muted cr-small"
    >
      {{ ctx.t('costReport.usage.note', { step: stepText(series.step), count: series.covered, zone: zone || '—' }) }}
      <span
        v-if="missing > 0"
        class="cr-gap-l"
      >{{ ctx.t('costReport.usage.missing', { count: missing }) }}</span>
      <span
        v-if="chart.hasPartial"
        class="cr-gap-l"
      >{{ ctx.t('costReport.usage.partial') }}</span>
    </p>
  </CrSection>
</template>
