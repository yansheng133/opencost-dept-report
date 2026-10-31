<script lang="ts">
// 儲存空間：申請了多少、用了多少. Not measured is shown as such, never as 0.
import { defineComponent, PropType, computed, ref } from 'vue';
import CrSection from '../CrSection.vue';
import Help from '../Help.vue';
import DeptTag from '../DeptTag.vue';
import Eff from '../Eff.vue';
import EffLegend from '../EffLegend.vue';
import MoreRow from '../MoreRow.vue';
import { useReportContext } from '../../composables/useReportContext';
import { filterStorage } from '../../utils/scope';
import { storageNotes } from '../../utils/actions';
import { storageNoteText } from '../../utils/describe';
import { capRows, fmtBytes } from '../../utils/format';
import { TABLE_LIMITS } from '../../config/backend';
import type { Report } from '../../utils/types';
import { fmtMoney } from '../../utils/money';

export default defineComponent({
  name: 'CostReportStorage',

  components: {
    CrSection, Help, DeptTag, Eff, EffLegend, MoreRow
  },

  props: {
    report: { type: Object as PropType<Report>, required: true },
    filter: { type: String, required: true },
  },

  setup(props) {
    const ctx = useReportContext();
    const open = ref(false);
    const rows = computed(() => filterStorage(props.report.storage?.rows || [], props.filter));
    const shown = computed(() => capRows(rows.value, TABLE_LIMITS.storage, open.value).shown);
    const notes = computed(() => storageNotes(props.report.storage, rows.value).map((n) => storageNoteText(ctx.value.t, n)));
    const special = (d: { special?: string | null }) => (d.special ? ctx.value.t(`costReport.storage.special.${ d.special }`) : '');

    const summary = computed(() => ctx.value.t('costReport.storage.summary', {
      count: rows.value.length, amount: fmtMoney(rows.value.reduce((s, r) => s + (r.cost || 0), 0)), currency: ctx.value.currency
    }));

    return {
      ctx, open, rows, shown, notes, special, fmtBytes, fmtMoney, summary, limit: TABLE_LIMITS.storage
    };
  },
});
</script>

<template>
  <CrSection
    id="storage"
    :title="ctx.t('costReport.storage.title')"
    :summary="summary"
    :description="ctx.t('costReport.storage.description')"
  >
    <EffLegend />
    <div class="cr-table-wrap">
      <table class="sortable-table cr-table">
        <thead>
          <tr>
            <th><Help :text="ctx.t('costReport.storage.col.pvc')" /></th>
            <th><Help :text="ctx.t('costReport.storage.col.dept')" /></th>
            <th>
              <Help
                :text="ctx.t('costReport.storage.col.capacity')"
                :tip="ctx.t('costReport.help.capacity')"
              />
            </th>
            <th>
              <Help
                :text="ctx.t('costReport.storage.col.used')"
                :tip="ctx.t('costReport.help.used')"
              />
            </th>
            <th><Help :text="ctx.t('costReport.storage.col.usedPct')" /></th>
            <th>
              <Help
                :text="ctx.t('costReport.common.inCurrency', { label: ctx.t('costReport.storage.col.amount'), currency: ctx.currency })"
                :tip="ctx.t('costReport.help.storageAmount')"
              />
            </th>
            <th>
              <Help
                :text="ctx.t('costReport.common.inCurrency', { label: ctx.t('costReport.storage.col.idle'), currency: ctx.currency })"
                :tip="ctx.t('costReport.help.idle')"
              />
            </th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="r in shown"
            :key="r.pv"
          >
            <td>
              {{ r.pvc || r.pv }}
              <div class="text-muted cr-small">
                {{ r.ns || '' }}
              </div>
            </td>
            <td>
              <DeptTag
                v-for="d in r.depts"
                :id="d.id"
                :key="d.id"
                :label="special(d)"
              />
            </td>
            <td class="cr-num">
              {{ fmtBytes(r.capacityBytes) }}
            </td>
            <td
              class="cr-num"
              :class="{ 'text-muted': r.peakBytes === null }"
            >
              {{ r.peakBytes === null ? ctx.t('costReport.storage.notMeasured') : `${ fmtBytes(r.peakBytes) }${ r.partial ? '＋' : '' }` }}
              <div
                v-if="r.over"
                class="cr-warn cr-small"
              >
                {{ ctx.t('costReport.storage.over') }}
              </div>
            </td>
            <td>
              <span
                v-if="r.usedPct === null"
                class="text-muted"
              >—</span>
              <Eff
                v-else
                :value="r.usedPct"
              />
            </td>
            <td class="cr-num">
              {{ fmtMoney(r.cost) }}
            </td>
            <td class="cr-num">
              {{ r.idleCost === null ? '—' : fmtMoney(r.idleCost) }}
            </td>
          </tr>
          <tr v-if="!rows.length">
            <td
              colspan="7"
              class="text-muted"
            >
              {{ filter === 'all' ? ctx.t('costReport.storage.empty') : ctx.t('costReport.storage.emptyDept', { name: ctx.depts.name(filter) }) }}
            </td>
          </tr>
          <MoreRow
            :total="rows.length"
            :limit="limit"
            :open="open"
            :cols="7"
            @toggle="open = !open"
          />
        </tbody>
      </table>
    </div>
    <div class="cr-notes">
      <div
        v-for="n in notes"
        :key="n.title"
        class="cr-note"
      >
        <b>{{ n.title }}</b>
        <span>{{ n.text }}</span>
      </div>
    </div>
  </CrSection>
</template>
