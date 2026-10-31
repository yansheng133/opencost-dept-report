<script lang="ts">
// 計價依據: the rates in effect, where they come from (rate card version)
// and whether they match what OpenCost actually charged.
import { defineComponent, PropType, computed } from 'vue';
import CrSection from '../CrSection.vue';
import Help from '../Help.vue';
import { useReportContext } from '../../composables/useReportContext';
import { n4 } from '../../utils/format';
import type { Report } from '../../utils/types';

type Key = 'cpu' | 'ram' | 'storage';

export default defineComponent({
  name: 'CostReportRates',

  components: { CrSection, Help },

  props: { report: { type: Object as PropType<Report>, required: true } },

  setup(props) {
    const ctx = useReportContext();
    const sum = (k: 'cpuCoreHours' | 'ramGiBHours' | 'pvGiBHours' | 'cpu' | 'ram' | 'pv') => props.report.depts.reduce((s, d) => s + (d[k] || 0), 0);
    const rows = computed(() => {
      const { t } = ctx.value;
      const check = props.report.rateCard?.check || {};
      const mark = (k: Key) => {
        const c = check[k];

        if (!c || c.status === 'unknown') {
          return { text: '', bad: false };
        }
        if (c.status === 'ok') {
          return { text: t('costReport.rates.checkOk'), bad: false };
        }
        if (c.status === 'minor') {
          return { text: t('costReport.rates.checkMinor', c as unknown as Record<string, unknown>), bad: false };
        }

        return { text: t('costReport.rates.checkMismatch', { implied: c.implied }), bad: true };
      };
      const r = props.report.rates;

      return ([
        ['cpu', r.cpu, t('costReport.rates.usageCpu', { v: sum('cpuCoreHours').toFixed(4) }), sum('cpu')],
        ['ram', r.ram, t('costReport.rates.usageGiB', { v: sum('ramGiBHours').toFixed(4) }), sum('ram')],
        ['storage', r.storage, t('costReport.rates.usageGiB', { v: sum('pvGiBHours').toFixed(4) }), sum('pv')],
      ] as [Key, number | null, string, number][]).map(([k, rate, usage, amount]) => ({
        k, rate: rate === null || rate === undefined ? '—' : String(rate), usage, amount: n4(amount), mark: mark(k)
      }));
    });
    const card = computed(() => {
      const { t } = ctx.value;
      const rc = props.report.rateCard;

      if (!rc?.loaded) {
        return [t('costReport.rates.cardMissing')];
      }
      const v = rc.version || {};

      return [
        t('costReport.rates.card', { count: (rc.versions || []).length, effective: v.effective || '—' }),
        v.reason ? t('costReport.rates.cardReason', { reason: v.reason }) : '',
        v.author ? t('costReport.rates.cardAuthor', { author: v.author }) : '',
        rc.note || '',
        t('costReport.rates.cardNote'),
      ].filter(Boolean);
    });

    return {
      ctx, rows, card, n4
    };
  },
});
</script>

<template>
  <CrSection
    id="rates"
    :title="ctx.t('costReport.rates.title')"
    :description="ctx.t('costReport.rates.description')"
  >
    <div class="cr-card-info">
      <p
        v-for="(line, i) in card"
        :key="i"
      >
        {{ line }}
      </p>
    </div>
    <div class="cr-table-wrap">
      <table class="sortable-table cr-table">
        <thead>
          <tr>
            <th>{{ ctx.t('costReport.rates.col.resource') }}</th>
            <th>
              <Help
                :text="ctx.t('costReport.rates.col.rate')"
                :tip="ctx.t('costReport.help.rate')"
              />
            </th>
            <th>
              <Help
                :text="ctx.t('costReport.rates.col.unit')"
                :tip="ctx.t('costReport.help.unit')"
              />
            </th>
            <th>
              <Help
                :text="ctx.t('costReport.rates.col.usage')"
                :tip="ctx.t('costReport.help.usage')"
              />
            </th>
            <th>
              <Help
                :text="ctx.t('costReport.rates.col.amount')"
                :tip="ctx.t('costReport.help.amount')"
              />
            </th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="r in rows"
            :key="r.k"
          >
            <td>{{ ctx.t(`costReport.resource.${ r.k }`) }}</td>
            <td class="cr-num">
              {{ r.rate }}
              <span
                v-if="r.mark.text"
                class="cr-small"
                :class="r.mark.bad ? 'cr-bad' : 'text-muted'"
              >{{ r.mark.text }}</span>
            </td>
            <td>{{ ctx.t(`costReport.rates.unit.${ r.k }`) }}</td>
            <td class="cr-num">
              {{ r.usage }}
            </td>
            <td class="cr-num">
              {{ r.amount }}
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <div class="cr-notes">
      <div class="cr-note">
        <b>{{ ctx.t('costReport.rates.notes.maxTitle') }}</b>
        <span>{{ ctx.t('costReport.rates.notes.max') }}</span>
      </div>
      <div
        v-if="report.scope && report.scope.all"
        class="cr-note"
      >
        <b>{{ ctx.t('costReport.rates.notes.consistentTitle') }}</b>
        <span>{{ ctx.t('costReport.rates.notes.consistent', { total: n4(report.total) }) }}</span>
      </div>
    </div>
  </CrSection>
</template>
