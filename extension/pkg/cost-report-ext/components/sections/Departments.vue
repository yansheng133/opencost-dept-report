<script lang="ts">
// 各部門應分攤金額: bars and the table per department. Idle is either its
// own row (absorbed by the platform) or spread into each department's
// amount (`shared`); the total is the same either way.
import { defineComponent, PropType, computed } from 'vue';
import CrSection from '../CrSection.vue';
import Bars, { BarRow } from '../Bars.vue';
import Help from '../Help.vue';
import DeptTag from '../DeptTag.vue';
import { useReportContext } from '../../composables/useReportContext';
import { visibleParts, IDLE_COLOR, UNALLOCATED_COLOR } from '../../utils/scope';
import { n4, share } from '../../utils/format';
import type { Report } from '../../utils/types';
import { fmtMoney, fmtMonthly } from '../../utils/money';

export default defineComponent({
  name: 'CostReportDepartments',

  components: {
    CrSection, Bars, Help, DeptTag
  },

  props: {
    report:     { type: Object as PropType<Report>, required: true },
    filter:     { type: String, required: true },
    idleShared: { type: Boolean, required: true },
    month:      { type: Number, required: true },
    thin:       { type: Boolean, required: true },
  },

  setup(props) {
    const ctx = useReportContext();
    const parts = computed(() => visibleParts(props.report));
    const rows = computed(() => props.report.depts
      .filter((d) => props.filter === 'all' || d.id === props.filter)
      // The idle share is its own column so the row adds up: CPU + memory +
      // storage + shared idle = subtotal (without it a finance reader adds
      // the columns and the subtotal does not match).
      .map((d) => ({
        ...d, amount: props.idleShared ? d.shared : d.total, idleShare: props.idleShared ? d.shared - d.total : 0
      })));
    const sum = computed(() => rows.value.reduce((s, r) => s + r.amount, 0));
    const idleSum = computed(() => rows.value.reduce((s, r) => s + r.idleShare, 0));
    // Departments are compared with each other on their own scale; idle and
    // unallocated (often far larger) go below a divider on another scale,
    // or the departments shrink to a few pixels each.
    const bars = computed<BarRow[]>(() => {
      const { t, depts } = ctx.value;
      const total = props.report.total;

      return rows.value.map((r) => ({
        key:   r.id,
        label: depts.name(r.id),
        color: depts.color(r.id),
        value: r.amount,
        note:  share(r.amount, total),
        tip:   t('costReport.dept.bar', {
          name: depts.name(r.id), amount: fmtMoney(r.amount), month: fmtMonthly(r.amount * props.month)
        }),
      }));
    });
    const otherBars = computed<BarRow[]>(() => {
      const { t } = ctx.value;
      const total = props.report.total;
      const out: BarRow[] = [];
      const others = props.filter === 'all';

      if (others && parts.value.idle && !props.idleShared) {
        out.push({
          key: '__idle__', label: t('costReport.totals.idle'), color: IDLE_COLOR, value: props.report.idle || 0, hatch: true, note: share(props.report.idle, total), tip: t('costReport.dept.idleBar')
        });
      }
      if (others && parts.value.unallocated) {
        const u = props.idleShared ? props.report.unallocated!.shared : props.report.unallocated!.separate;

        out.push({
          key: '__unallocated__', label: t('costReport.totals.unallocated'), color: UNALLOCATED_COLOR, value: u, note: share(u, total), tip: t('costReport.dept.unallocBar', { label: ctx.value.label })
        });
      }

      return out;
    });
    const rate = (v: number | null) => (v === null || v === undefined ? '—' : String(v));

    return {
      ctx,
      rows,
      sum,
      idleSum,
      bars,
      otherBars,
      fmtMoney,
      fmtMonthly,
      n4,
      rate,
      summary: computed(() => ctx.value.t('costReport.dept.summary', {
        amount: fmtMoney(sum.value), currency: ctx.value.currency, count: rows.value.length
      })),
      cur: (label: string) => ctx.value.t('costReport.common.inCurrency', { label, currency: ctx.value.currency })
    };
  },
});
</script>

<template>
  <CrSection
    id="dept"
    :title="ctx.t('costReport.dept.title')"
    :description="ctx.t('costReport.dept.description')"
    :summary="summary"
  >
    <p class="cr-small text-muted">
      {{ idleShared ? ctx.t('costReport.dept.hintShared') : ctx.t('costReport.dept.hintSeparate') }}
    </p>
    <Bars :rows="bars" />
    <template v-if="otherBars.length">
      <div class="cr-bars-divider text-muted cr-small">
        {{ ctx.t('costReport.dept.otherScale') }}
      </div>
      <Bars :rows="otherBars" />
    </template>
    <div class="cr-table-wrap">
      <table class="sortable-table cr-table">
        <thead>
          <tr>
            <th><Help :text="ctx.t('costReport.dept.col.dept')" /></th>
            <th>
              <Help
                :text="ctx.t('costReport.dept.col.cpuHours')"
                :tip="ctx.t('costReport.help.cpuHours')"
              />
            </th>
            <th>
              <Help
                :text="cur(ctx.t('costReport.dept.col.cpuCost'))"
                :tip="ctx.t('costReport.help.cpuCost')"
              />
            </th>
            <th>
              <Help
                :text="ctx.t('costReport.dept.col.ramHours')"
                :tip="ctx.t('costReport.help.ramHours')"
              />
            </th>
            <th>
              <Help
                :text="cur(ctx.t('costReport.dept.col.ramCost'))"
                :tip="ctx.t('costReport.help.ramCost')"
              />
            </th>
            <th>
              <Help
                :text="cur(ctx.t('costReport.dept.col.pvCost'))"
                :tip="ctx.t('costReport.help.pvCost')"
              />
            </th>
            <th v-if="idleShared">
              <Help
                :text="cur(ctx.t('costReport.dept.col.sharedIdle'))"
                :tip="ctx.t('costReport.help.sharedIdle')"
              />
            </th>
            <th>
              <Help
                :text="cur(ctx.t('costReport.dept.col.subtotal'))"
                :tip="ctx.t('costReport.help.subtotal')"
              />
            </th>
            <th :class="{ 'cr-thin': thin }">
              <Help
                :text="cur(ctx.t('costReport.dept.col.month'))"
                :tip="ctx.t('costReport.help.month')"
              />
            </th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="r in rows"
            :key="r.id"
          >
            <td>
              <DeptTag :id="r.id" />
              <span
                v-if="r.basis === 'mixed'"
                class="cr-pill ml-5"
              >{{ ctx.t('costReport.dept.basisMixed') }}</span>
            </td>
            <td class="cr-num">
              {{ n4(r.cpuCoreHours) }}
            </td>
            <td class="cr-num">
              {{ fmtMoney(r.cpu) }}
            </td>
            <td class="cr-num">
              {{ n4(r.ramGiBHours) }}
            </td>
            <td class="cr-num">
              {{ fmtMoney(r.ram) }}
            </td>
            <td class="cr-num">
              {{ fmtMoney(r.pv) }}
            </td>
            <td
              v-if="idleShared"
              class="cr-num"
            >
              {{ fmtMoney(r.idleShare) }}
            </td>
            <td class="cr-num">
              {{ fmtMoney(r.amount) }}
            </td>
            <td
              class="cr-num"
              :class="{ 'cr-thin': thin }"
            >
              {{ fmtMonthly(r.amount * month) }}
            </td>
          </tr>
        </tbody>
        <tfoot>
          <tr>
            <td>{{ idleShared ? ctx.t('costReport.dept.sumShared') : ctx.t('costReport.dept.sum') }}</td>
            <td /><td /><td /><td /><td />
            <td
              v-if="idleShared"
              class="cr-num"
            >
              {{ fmtMoney(idleSum) }}
            </td>
            <td class="cr-num">
              {{ fmtMoney(sum) }}
            </td>
            <td
              class="cr-num"
              :class="{ 'cr-thin': thin }"
            >
              {{ fmtMonthly(sum * month) }}
            </td>
          </tr>
        </tfoot>
      </table>
    </div>
    <p class="cr-footnote">
      {{ ctx.t('costReport.common.roundingNote') }}
    </p>
    <p class="cr-formula">
      <span class="text-muted">{{ ctx.t('costReport.dept.formula') }}</span>
      <span class="cr-num">{{ ctx.t('costReport.dept.formulaText', { cpu: rate(report.rates.cpu), ram: rate(report.rates.ram), storage: rate(report.rates.storage) }) }}{{ idleShared ? ctx.t('costReport.dept.formulaShared') : '' }}</span>
    </p>
  </CrSection>
</template>
