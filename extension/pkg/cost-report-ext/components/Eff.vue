<script lang="ts">
// A small bar for a ratio (efficiency, usage): full at 100%, highlighted
// above it, warned below 10%.
import { defineComponent } from 'vue';
import { pct } from '../utils/format';

export default defineComponent({
  name: 'CostReportEff',

  props: {
    value: { type: Number, required: true },
    color: { type: String, default: 'var(--primary)' },
  },

  setup(props) {
    const fill = () => (props.value >= 1 ? 'var(--success)' : props.value < 0.1 ? 'var(--warning)' : props.color);

    return { pct, fill };
  },
});
</script>

<template>
  <span class="cr-eff">
    <span class="cr-eff-track"><span
      class="cr-eff-fill"
      :style="{ width: `${ Math.min(value, 1) * 100 }%`, background: fill() }"
    /></span>
    <span class="cr-num">{{ pct(value) }}</span>
  </span>
</template>
