<script lang="ts">
// Horizontal bars, one per row, scaled to the largest value. Hatched bars
// are the ones that are not a department's own cost (idle, other
// environments). The value and its note are text, not only the bar.
import { defineComponent, PropType, computed } from 'vue';
import { money } from '../utils/format';

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
    const width = (v: number) => `${ Math.max(v / max.value * 100, 0.6) }%`;

    return { width, money };
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
        <span :title="r.label">{{ r.label }}</span>
      </div>
      <div class="cr-bar-track">
        <div
          class="cr-bar-fill"
          :class="{ hatch: r.hatch }"
          :style="{ width: width(r.value), background: r.color }"
        />
      </div>
      <div class="cr-bar-val cr-num">
        {{ money(r.value) }}<span
          v-if="r.note"
          class="text-muted cr-gap-l"
        >{{ r.note }}</span>
      </div>
    </div>
  </div>
</template>
