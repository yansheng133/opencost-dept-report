<script lang="ts">
// 每個工作負載付了多少、用了多少, with observations picked from the data.
import { defineComponent, PropType, computed, ref } from 'vue';
import CrSection from '../CrSection.vue';
import Help from '../Help.vue';
import DeptTag from '../DeptTag.vue';
import Eff from '../Eff.vue';
import MoreRow from '../MoreRow.vue';
import { useReportContext } from '../../composables/useReportContext';
import { filterWorkloads } from '../../utils/scope';
import { workloadNotes } from '../../utils/actions';
import { workloadNoteText } from '../../utils/describe';
import { capRows, money } from '../../utils/format';
import { TABLE_LIMITS } from '../../config/backend';
import type { Report } from '../../utils/types';

export default defineComponent({
  name: 'CostReportWorkloads',

  components: {
    CrSection, Help, DeptTag, Eff, MoreRow
  },

  props: {
    report: { type: Object as PropType<Report>, required: true },
    filter: { type: String, required: true },
  },

  setup(props) {
    const ctx = useReportContext();
    const open = ref(false);
    const list = computed(() => filterWorkloads(props.report.workloads || [], props.filter));
    const shown = computed(() => capRows(list.value, TABLE_LIMITS.workloads, open.value).shown);
    const notes = computed(() => workloadNotes(list.value).map((n) => workloadNoteText(ctx.value.t, n)));

    const summary = computed(() => ctx.value.t('costReport.workloads.summary', { count: list.value.length }));

    return {
      ctx, open, list, shown, notes, money, summary, limit: TABLE_LIMITS.workloads
    };
  },
});
</script>

<template>
  <CrSection
    id="workloads"
    :title="ctx.t('costReport.workloads.title')"
    :summary="summary"
    :description="ctx.t('costReport.workloads.description')"
  >
    <div class="cr-table-wrap">
      <table class="sortable-table cr-table">
        <thead>
          <tr>
            <th>
              <Help
                :text="ctx.t('costReport.workloads.col.workload')"
                :tip="ctx.t('costReport.help.workload')"
              />
            </th>
            <th><Help :text="ctx.t('costReport.workloads.col.dept')" /></th>
            <th>
              <Help
                :text="ctx.t('costReport.workloads.col.env')"
                :tip="ctx.t('costReport.help.env')"
              />
            </th>
            <th>
              <Help
                :text="ctx.t('costReport.workloads.col.cpu')"
                :tip="ctx.t('costReport.help.cpu')"
              />
            </th>
            <th>
              <Help
                :text="ctx.t('costReport.workloads.col.cpuEff')"
                :tip="ctx.t('costReport.help.cpuEff')"
              />
            </th>
            <th>
              <Help
                :text="ctx.t('costReport.workloads.col.ram')"
                :tip="ctx.t('costReport.help.ram')"
              />
            </th>
            <th>
              <Help
                :text="ctx.t('costReport.workloads.col.ramEff')"
                :tip="ctx.t('costReport.help.ramEff')"
              />
            </th>
            <th>
              <Help
                :text="ctx.t('costReport.common.inCurrency', { label: ctx.t('costReport.workloads.col.amount'), currency: ctx.currency })"
                :tip="ctx.t('costReport.help.amount')"
              />
            </th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="w in shown"
            :key="`${ w.ns }/${ w.kind }/${ w.name }/${ w.dept }/${ w.env }`"
          >
            <td>
              {{ w.name }}
              <div class="text-muted cr-small">
                {{ w.ns }}
              </div>
            </td>
            <td><DeptTag :id="w.dept" /></td>
            <td>
              <span
                class="cr-pill"
                :class="{ none: !w.env }"
              >{{ w.env || ctx.t('costReport.common.untagged') }}</span>
            </td>
            <td class="cr-num">
              {{ w.cpuReq }} → {{ w.cpuUse }}
            </td>
            <td>
              <Eff
                :value="w.cpuEff"
                :color="ctx.depts.color(w.dept)"
              />
            </td>
            <td class="cr-num">
              {{ w.ramReqMi }} → {{ w.ramUseMi }} MiB
            </td>
            <td>
              <Eff
                :value="w.ramEff"
                :color="ctx.depts.color(w.dept)"
              />
            </td>
            <td class="cr-num">
              {{ money(w.total) }}
            </td>
          </tr>
          <tr v-if="!list.length">
            <td
              colspan="8"
              class="text-muted"
            >
              {{ ctx.t('costReport.workloads.empty') }}
            </td>
          </tr>
          <MoreRow
            :total="list.length"
            :limit="limit"
            :open="open"
            :cols="8"
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
