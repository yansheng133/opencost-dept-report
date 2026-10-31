<script lang="ts">
// 帳期: the sealed days and the monthly roll-up made from them (the source
// is the seals, not Prometheus, so months older than its retention are
// still there and do not change). A day leads to that day's report.
import {
  defineComponent, ref, computed, watch, onMounted
} from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import { useBackend } from '../composables/useBackend';
import { provideReportContext } from '../composables/useReportContext';
import TargetBar from '../components/TargetBar.vue';
import Help from '../components/Help.vue';
import MoreRow from '../components/MoreRow.vue';
import { getSeals, getPeriods, ApiError } from '../utils/api';
import { monthDepts } from '../utils/scope';
import { monthReasonText, missingText } from '../utils/describe';
import { capRows, pctValue, fmtLocal, timeZone } from '../utils/format';
import { firstSealAfter, utcMidnightLocal } from '../utils/day';
import { reportRoute } from '../config/product';
import { TABLE_LIMITS } from '../config/backend';
import type { Month, SealRow, Scope } from '../utils/types';
import { fmtMoney, setMoneyBasis } from '../utils/money';

export default defineComponent({
  name: 'CostReportPeriods',

  components: {
    TargetBar, Help, MoreRow
  },

  setup() {
    const { t } = useText(useStore());
    const backend = useBackend();
    const config = computed(() => backend.selected.value?.config || null);
    const ctx = provideReportContext(t, config);
    const seals = ref<SealRow[]>([]);
    const months = ref<Month[]>([]);
    const enabled = ref(true);
    const scope = ref<Scope | null>(null);
    const error = ref<ApiError | null>(null);
    const loading = ref(false);
    const open = ref(false);
    const clusterId = computed(() => (backend.selected.value?.access === 'ready' ? backend.selected.value.clusterId : null));
    let seq = 0;

    async function load() {
      const c = clusterId.value;

      if (!c) {
        return;
      }
      const mine = ++seq;

      loading.value = true;
      try {
        // The two are independent; one failing still shows the other.
        const [s, p] = await Promise.allSettled([getSeals(c), getPeriods(c)]);

        if (mine !== seq) {
          return;
        }
        error.value = null;
        if (s.status === 'fulfilled') {
          seals.value = [...(s.value.seals || [])].reverse();
          enabled.value = s.value.enabled !== false;
          scope.value = s.value.scope || null;
        } else {
          seals.value = [];
          error.value = s.reason instanceof ApiError ? s.reason : null;
        }
        if (p.status === 'fulfilled') {
          months.value = [...(p.value.months || [])].reverse();
          // One precision for the page, from the largest month.
          setMoneyBasis(Math.max(0, ...months.value.map((m) => Math.abs(m.total || 0))));
        } else {
          months.value = [];
          error.value = error.value || (p.reason instanceof ApiError ? p.reason : null);
        }
      } finally {
        if (mine === seq) {
          loading.value = false;
        }
      }
    }

    watch(clusterId, () => load(), { immediate: true });
    onMounted(() => backend.ensure());

    const thisMonth = new Date().toISOString().slice(0, 7);
    const monthView = computed(() => months.value.map((m) => ({
      ...m,
      reasonText:  monthReasonText(t, m),
      deptList:    monthDepts(m),
      missingList: missingText(t, m.missingDays || []),
      current:     m.month === thisMonth,
    })));
    const shown = computed(() => capRows(seals.value, TABLE_LIMITS.seals, open.value).shown);

    return {
      t,
      ctx,
      clusterId,
      seals,
      shown,
      open,
      monthView,
      enabled,
      scope,
      error,
      loading,
      fmtMoney,
      // When the first seal can appear, said once for both lists: from the
      // backend's earliest data when known, else the rule.
      firstSeal: computed(() => {
        const since = config.value?.dataSince;
        const at = firstSealAfter(since);
        const zone = timeZone() || t('costReport.report.localTime');

        if (at === null || !since) {
          return t('costReport.periods.firstSealRule', { time: utcMidnightLocal() });
        }
        // Why not earlier: the day the data starts in is only partly
        // covered and is not sealed; the next full UTC day is.
        const firstDay = new Date(at - 864e5).toISOString().slice(0, 10);
        const partial = Date.parse(since) % 864e5 !== 0 ? since.slice(0, 10) : '';
        const when = t('costReport.periods.firstSealAt', {
          day: firstDay.slice(5), time: fmtLocal(new Date(at).toISOString(), true), zone
        });

        return partial ? `${ t('costReport.periods.partialDay', { day: partial.slice(5) }) }${ when }` : when;
      }),
      pctValue,
      reportRoute,
      limit: TABLE_LIMITS.seals,
    };
  },
});
</script>

<template>
  <div class="cr-page">
    <header class="cr-header">
      <div>
        <h1>{{ t('costReport.periods.title') }}</h1>
        <p class="text-muted">
          {{ t('costReport.periods.description') }}
        </p>
      </div>
      <TargetBar />
    </header>

    <template v-if="clusterId">
      <div
        v-if="error && error.code === 'no_department'"
        class="cr-banner cr-banner-info"
      >
        {{ t('costReport.target.noDepartment') }}
      </div>
      <div
        v-else-if="error"
        class="cr-banner cr-banner-error"
      >
        {{ t(`costReport.errors.${ error.code }`) }} <span class="text-muted cr-small">({{ error.message }})</span>
      </div>
      <div
        v-if="!enabled"
        class="cr-banner cr-banner-info"
      >
        {{ t('costReport.periods.disabled') }}
      </div>
      <p
        v-if="scope && !scope.all"
        class="cr-scope"
      >
        {{ t('costReport.periods.scoped') }}
      </p>

      <div
        v-if="enabled && !loading && !seals.length"
        class="cr-banner cr-banner-info"
      >
        {{ firstSeal }}
      </div>

      <section
        id="months"
        class="cr-section"
      >
        <h2>{{ t('costReport.periods.months') }}</h2>
        <p
          v-if="loading && !monthView.length"
          class="text-muted"
        >
          {{ t('costReport.common.loading') }}
        </p>
        <p
          v-else-if="!monthView.length"
          class="text-muted"
        >
          {{ t('costReport.periods.monthsEmpty') }}
        </p>
        <div
          v-for="m in monthView"
          :key="m.month"
          class="cr-month"
        >
          <div class="cr-month-top">
            <span class="cr-month-ym cr-num">{{ m.month }}</span>
            <span
              class="cr-tag"
              :class="m.verdict"
            >{{ t(`costReport.periods.verdict.${ m.verdict }`) }}</span>
            <span class="cr-month-amt cr-num">{{ fmtMoney(m.total) }}</span>
          </div>
          <div
            v-if="m.reasonText"
            class="cr-small"
          >
            {{ m.reasonText }}
          </div>
          <div
            v-if="m.deptList.length"
            class="cr-month-depts"
          >
            <span
              v-for="d in m.deptList"
              :key="d.id"
            ><span
              class="cr-swatch"
              :style="{ background: ctx.depts.color(d.id) }"
            />{{ ctx.depts.name(d.id) }} <b class="cr-num">{{ fmtMoney(d.total) }}</b></span>
          </div>
          <div class="text-muted cr-small">
            {{ t('costReport.periods.sealedDays', { sealed: m.sealedDays, expected: m.expectedDays }) }}<span v-if="m.current">{{ t('costReport.periods.notOver', { days: m.monthDays }) }}</span>
            <span v-if="m.minCoverage !== null && m.minCoverage !== undefined"> · {{ t('costReport.periods.minCoverage', { pct: pctValue(m.minCoverage) }) }}</span>
            <span v-if="m.gapMinutes"> · {{ t('costReport.periods.gapMinutes', { minutes: m.gapMinutes }) }}</span>
            <span v-if="m.estimatedPct"> · {{ t('costReport.periods.estimated', { pct: pctValue(m.estimatedPct) }) }}</span>
          </div>
          <div
            v-if="m.missingList"
            class="cr-warn cr-small"
          >
            {{ t('costReport.periods.missing', { ranges: m.missingList }) }}
          </div>
        </div>
      </section>

      <section
        id="days"
        class="cr-section"
      >
        <h2>{{ t('costReport.periods.days') }}</h2>
        <div class="cr-table-wrap">
          <table class="sortable-table cr-table">
            <thead>
              <tr>
                <th>
                  <Help
                    :text="t('costReport.periods.col.day')"
                    :tip="t('costReport.help.day')"
                  />
                </th>
                <th>
                  <Help
                    :text="t('costReport.periods.col.total')"
                    :tip="t('costReport.help.sealTotal')"
                  />
                </th>
                <th>
                  <Help
                    :text="t('costReport.periods.col.coverage')"
                    :tip="t('costReport.help.coverage')"
                  />
                </th>
                <th>
                  <Help
                    :text="t('costReport.periods.col.estimated')"
                    :tip="t('costReport.help.estimated')"
                  />
                </th>
                <th>
                  <Help
                    :text="t('costReport.periods.col.gate')"
                    :tip="t('costReport.help.gate')"
                  />
                </th>
                <th>
                  <Help
                    :text="t('costReport.periods.col.intact')"
                    :tip="t('costReport.help.intact')"
                  />
                </th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="s in shown"
                :key="s.day"
              >
                <td>
                  <router-link :to="reportRoute(s.day)">
                    {{ s.day }}
                  </router-link>
                </td>
                <td class="cr-num">
                  {{ s.total === null || s.total === undefined ? '—' : fmtMoney(s.total) }}
                </td>
                <td class="cr-num">
                  {{ pctValue(s.coveragePct) }}
                </td>
                <td class="cr-num">
                  {{ pctValue(s.estimatedPct) }}
                </td>
                <td>
                  <span
                    v-if="s.gate"
                    class="cr-tag"
                    :class="s.gate"
                  >{{ t(`costReport.periods.verdict.${ s.gate }`) }}</span>
                  <span v-else>—</span>
                </td>
                <td>
                  <b
                    v-if="s.status === 'unreadable'"
                    class="cr-bad"
                  >✗ {{ t('costReport.periods.unreadable') }}</b>
                  <span v-else-if="s.intact">✓ {{ t('costReport.periods.intact') }}</span>
                  <b
                    v-else-if="s.intact === false"
                    class="cr-bad"
                  >✗ {{ t('costReport.periods.tampered') }}</b>
                  <span v-else>—</span>
                </td>
              </tr>
              <tr v-if="!seals.length && !loading">
                <td
                  colspan="6"
                  class="text-muted"
                >
                  {{ t('costReport.periods.daysEmpty') }}
                </td>
              </tr>
              <MoreRow
                :total="seals.length"
                :limit="limit"
                :open="open"
                :cols="6"
                @toggle="open = !open"
              />
            </tbody>
          </table>
        </div>
      </section>
    </template>
  </div>
</template>

<style lang="scss">
@import '../styles/report.scss';
</style>
