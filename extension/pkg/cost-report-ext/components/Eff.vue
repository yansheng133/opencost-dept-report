<script lang="ts">
// A small bar for a ratio (efficiency, usage): full at 100%, marked below
// 10% (barely used) and above 100% (above the request) in two colours. Using far more than requested is a capacity risk
// (the scheduler placed it by its request), not a good score.
import { defineComponent, PropType } from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import { pct } from '../utils/format';

export default defineComponent({
  name: 'CostReportEff',

  props: {
    // null: nothing was requested, so there is no ratio to draw.
    value: { type: Number as PropType<number | null>, default: null },
    color: { type: String, default: 'var(--primary)' },
  },

  setup(props) {
    const { t } = useText(useStore());
    const over = () => props.value !== null && props.value > 1;
    // Two different warnings, two different colours (EffLegend sits above
    // every table that draws these bars): above the request, and barely used.
    const under = () => props.value !== null && props.value < 0.1;
    const fill = () => (over() || under() ? undefined : props.color);

    return {
      pct, fill, over, under, t
    };
  },
});
</script>

<template>
  <span
    v-if="value === null"
    class="cr-eff text-muted cr-small"
  >{{ t('costReport.workloads.noRequest') }}</span>
  <span
    v-else
    class="cr-eff"
    :class="{ over: over(), under: under() }"
    :title="over() ? t('costReport.workloads.overRequest') : undefined"
  >
    <span class="cr-eff-track"><span
      class="cr-eff-fill"
      :class="{ some: value > 0 }"
      :style="{ width: `${ Math.min(value, 1) * 100 }%`, background: fill() }"
    /></span>
    <span class="cr-num">{{ pct(value) }}</span>
  </span>
</template>
