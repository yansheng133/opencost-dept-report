<script lang="ts">
// Horizontal bars, one per row, scaled to the largest value. Hatched bars
// are the ones that are not a department's own cost (idle, other
// environments). The value and its note are text, not only the bar.
import { defineComponent, PropType, computed } from 'vue';
import { fmtMoney } from '../utils/money';

export interface BarRow {
  key: string;
  label: string;
  color: string;
  value: number;
  note?: string;
  hatch?: boolean;
  tip?: string;
}

export default defineComponent({
  name: 'CostReportBars',

  props: { rows: { type: Array as PropType<BarRow[]>, required: true } },

  setup(props) {
    const max = computed(() => Math.max(...props.rows.map((r) => r.value), 0.0001));
    // Zero draws nothing: a sliver would read as "a little".
    const width = (v: number) => (v > 0 ? `${ Math.max(v / max.value * 100, 0.6) }%` : '0');

    return { width, fmtMoney };
  },
});
</script>

<template>
  <div class="cr-bars">
    <div
      v-for="r in rows"
      :key="r.key"
      class="cr-bar-row"
      :title="r.tip || undefined"
    >
      <div class="cr-bar-name">
        <span
          class="cr-swatch"
          :style="{ background: r.color }"
        />
        <span
          class="cr-bar-label"
          :title="r.label"
        >{{ r.label }}</span>
      </div>
      <div class="cr-bar-track">
        <div
          v-if="r.value > 0"
          class="cr-bar-fill"
          :class="{ hatch: r.hatch }"
          :style="{ width: width(r.value), background: r.color }"
        />
      </div>
      <div class="cr-bar-val cr-num">
        {{ fmtMoney(r.value) }}<span
          v-if="r.note && r.value > 0"
          class="text-muted cr-gap-l"
        >{{ r.note }}</span>
      </div>
    </div>
  </div>
</template>
