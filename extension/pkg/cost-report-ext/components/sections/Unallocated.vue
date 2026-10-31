<script lang="ts">
// 無法分攤的成本: the largest namespaces without a department (full scope
// only). Platform components are marked: they need a sharing rule, not a
// department label.
import { defineComponent, PropType, computed } from 'vue';
import CrSection from '../CrSection.vue';
import Bars, { BarRow } from '../Bars.vue';
import { useReportContext } from '../../composables/useReportContext';
import { IDLE_COLOR } from '../../utils/scope';
import { share } from '../../utils/format';
import type { Report } from '../../utils/types';
import { fmtMoney } from '../../utils/money';

// Namespaces listed by name; the rest are one row.
const TOP = 10;

export default defineComponent({
  name: 'CostReportUnallocated',

  components: { CrSection, Bars },

  props: {
    report:     { type: Object as PropType<Report>, required: true },
    // The namespace amounts exist only on the separate basis; with idle
    // shared the section says so instead of silently disagreeing with the card.
    idleShared: { type: Boolean, default: false },
  },

  setup(props) {
    const ctx = useReportContext();
    const bars = computed<BarRow[]>(() => {
      const { t } = ctx.value;
      // Shares of the period's total, as everywhere else on the page (not of
      // the unallocated amount: two different 100% on one page misled).
      const whole = props.report.total || 0;

      const all = props.report.unallocRows || [];
      const top: BarRow[] = all.slice(0, TOP).map((r) => ({
        key:   r.ns,
        label: r.system ? `${ r.ns } · ${ t('costReport.unalloc.system') }` : r.ns,
        color: IDLE_COLOR,
        value: r.total,
        hatch: true,
        note:  share(r.total, whole),
        tip:   t('costReport.unalloc.bar', {
          ns: r.ns, amount: fmtMoney(r.total), pct: share(r.total, whole)
        }),
      }));
      // The rest in one row, so the shares add up to the whole instead of
      // leaving a reader to look for the missing percent.
      const rest = all.slice(TOP);

      if (rest.length) {
        const sum = rest.reduce((s, r) => s + r.total, 0);

        top.push({
          key: '__rest__', label: t('costReport.unalloc.rest', { count: rest.length }), color: IDLE_COLOR, value: sum, hatch: true, note: share(sum, whole), tip: t('costReport.unalloc.restTip', { list: rest.map((r) => r.ns).join(', ') })
        });
      }

      return top;
    });
    const summary = computed(() => ctx.value.t('costReport.unalloc.summarySeparate', { amount: fmtMoney(props.report.unallocated?.separate || 0), pct: share(props.report.unallocated?.separate, props.report.total) }));

    return {
      ctx, bars, summary, fmtMoney
    };
  },
});
</script>

<template>
  <CrSection
    id="unalloc"
    :title="ctx.t('costReport.unalloc.title')"
    :description="ctx.t('costReport.unalloc.description', { label: ctx.label })"
    :summary="summary"
    cluster-wide
  >
    <p
      v-if="idleShared"
      class="cr-small cr-warn"
    >
      {{ ctx.t('costReport.unalloc.basisSeparate', { shared: fmtMoney(report.unallocated ? report.unallocated.shared : 0), separate: fmtMoney(report.unallocated ? report.unallocated.separate : 0) }) }}
    </p>
    <Bars :rows="bars" />
    <p class="cr-footnote">
      {{ ctx.t('costReport.unalloc.pctNote') }}
    </p>
  </CrSection>
</template>
