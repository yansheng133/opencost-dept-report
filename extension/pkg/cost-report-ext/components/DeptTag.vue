<script lang="ts">
// A department with its color; untagged when there is none.
import { defineComponent, PropType } from 'vue';
import { useReportContext } from '../composables/useReportContext';

export default defineComponent({
  name: 'CostReportDeptTag',

  props: {
    id:    { type: String as PropType<string | null>, default: null },
    label: { type: String, default: '' },
  },

  setup() {
    return { ctx: useReportContext() };
  },
});
</script>

<template>
  <span
    v-if="id && !label"
    class="cr-dept-tag"
  >
    <span
      class="cr-swatch"
      :style="{ background: ctx.depts.color(id) }"
    />
    {{ ctx.depts.name(id) }}
  </span>
  <span
    v-else
    class="cr-pill"
  >{{ label || ctx.t('costReport.common.untagged') }}</span>
</template>
