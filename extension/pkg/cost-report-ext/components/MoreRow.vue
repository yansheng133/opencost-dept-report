<script lang="ts">
// The last row of a capped table: "show the other N rows" / "show fewer".
import { defineComponent } from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';

export default defineComponent({
  name: 'CostReportMoreRow',

  props: {
    // All rows and how many are shown when capped (utils/format capRows).
    total: { type: Number, required: true },
    limit: { type: Number, required: true },
    open:  { type: Boolean, required: true },
    cols:  { type: Number, required: true },
  },

  emits: ['toggle'],

  setup() {
    return useText(useStore());
  },
});
</script>

<template>
  <tr v-if="total > limit + 1">
    <td
      :colspan="cols"
      class="cr-more"
    >
      <button
        type="button"
        class="btn btn-sm role-link"
        @click="$emit('toggle')"
      >
        {{ open ? t('costReport.common.showLess') : t('costReport.common.showMore', { count: total - limit }) }}
      </button>
    </td>
  </tr>
</template>
