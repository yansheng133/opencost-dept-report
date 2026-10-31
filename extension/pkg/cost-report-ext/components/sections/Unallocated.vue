<script lang="ts">
// 無法分攤的成本: the largest namespaces without a department (full scope
// only). Platform components are marked: they need a sharing rule, not a
// department label.
import { defineComponent, PropType, computed } from 'vue';
import CrSection from '../CrSection.vue';
import Bars, { BarRow } from '../Bars.vue';
import { useReportContext } from '../../composables/useReportContext';
import { IDLE_COLOR } from '../../utils/scope';
import { n4, share } from '../../utils/format';
import type { Report } from '../../utils/types';

export default defineComponent({
  name: 'CostReportUnallocated',

  components: { CrSection, Bars },

  props: { report: { type: Object as PropType<Report>, required: true } },

  setup(props) {
    const ctx = useReportContext();
    const bars = computed<BarRow[]>(() => {
      const { t } = ctx.value;
      const whole = props.report.unallocated?.separate || 0;

      return (props.report.unallocRows || []).slice(0, 10).map((r) => ({
        key:   r.ns,
        label: r.system ? `${ r.ns } · ${ t('costReport.unalloc.system') }` : r.ns,
        color: IDLE_COLOR,
        value: r.total,
        hatch: true,
        note:  share(r.total, whole),
        tip:   t('costReport.unalloc.bar', {
          ns: r.ns, amount: n4(r.total), pct: share(r.total, whole)
        }),
      }));
    });

    return { ctx, bars };
  },
});
</script>

<template>
  <CrSection
    id="unalloc"
    :title="ctx.t('costReport.unalloc.title')"
    :description="ctx.t('costReport.unalloc.description', { label: ctx.label })"
  >
    <Bars :rows="bars" />
  </CrSection>
</template>
