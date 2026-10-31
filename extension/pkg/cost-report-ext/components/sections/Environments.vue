<script lang="ts">
// 各環境的成本: the department costs split again by the environment label.
// The first configured environment (normally prod) is solid; the others
// and workloads without the label are hatched.
import { defineComponent, PropType, computed } from 'vue';
import CrSection from '../CrSection.vue';
import Bars, { BarRow } from '../Bars.vue';
import { useReportContext } from '../../composables/useReportContext';
import { filterEnv } from '../../utils/scope';
import { money, moneyCol, share } from '../../utils/format';
import type { Report } from '../../utils/types';

export default defineComponent({
  name: 'CostReportEnvironments',

  components: { CrSection, Bars },

  props: {
    report: { type: Object as PropType<Report>, required: true },
    filter: { type: String, required: true },
  },

  setup(props) {
    const ctx = useReportContext();
    const rows = computed(() => filterEnv(props.report.ccEnv || [], props.filter));
    const envs = computed(() => {
      // The configured environments first, then any other value seen.
      const seen = new Set(ctx.value.environments);

      rows.value.forEach((r) => Object.keys(r.envs || {}).forEach((e) => seen.add(e)));

      return [...seen];
    });
    const bars = computed<BarRow[]>(() => {
      const { t, depts } = ctx.value;
      const out: BarRow[] = [];

      rows.value.forEach((r) => {
        envs.value.forEach((e, i) => {
          const v = r.envs?.[e] || 0;

          if (i >= ctx.value.environments.length && !v) {
            return;
          }
          out.push({
            key: `${ r.dept }/${ e }`, label: `${ depts.name(r.dept) } ${ e }`, color: depts.color(r.dept), value: v, hatch: i > 0
          });
        });
        if (r.other) {
          out.push({
            key: `${ r.dept }/__other__`, label: `${ depts.name(r.dept) } ${ t('costReport.env.other') }`, color: depts.color(r.dept), value: r.other, hatch: true
          });
        }
      });

      return out;
    });
    const note = computed(() => {
      const first = ctx.value.environments[0];
      const firstAmount = rows.value.reduce((s, r) => s + (r.envs?.[first] || 0), 0);
      const otherAmount = rows.value.reduce((s, r) => s + Object.entries(r.envs || {}).filter(([e]) => e !== first).reduce((a, [, v]) => a + v, 0), 0);

      return ctx.value.t('costReport.env.note', {
        first, firstAmount: money(firstAmount), otherAmount: money(otherAmount), share: share(otherAmount, firstAmount + otherAmount)
      });
    });

    return {
      ctx,
      rows,
      bars,
      note,
      summary: computed(() => {
        // Amount per environment, e.g. "prod 0.60 · dev 0.20 TWD".
        const per: [string, number][] = envs.value.map((e) => [e, rows.value.reduce((s, r) => s + (r.envs?.[e] || 0), 0)]);
        const other = rows.value.reduce((s, r) => s + (r.other || 0), 0);

        if (other) {
          per.push([ctx.value.t('costReport.env.other'), other]);
        }
        const fmt = moneyCol(per.map(([, v]) => v));

        return ctx.value.t('costReport.env.summary', { list: per.map(([e, v]) => `${ e } ${ fmt(v) }`).join(' · '), currency: ctx.value.currency });
      })
    };
  },
});
</script>

<template>
  <CrSection
    id="env"
    :title="ctx.t('costReport.env.title')"
    :summary="summary"
    :description="ctx.t('costReport.env.description', { label: ctx.envLabel })"
  >
    <Bars :rows="bars" />
    <p class="text-muted cr-small">
      {{ note }}
    </p>
  </CrSection>
</template>
