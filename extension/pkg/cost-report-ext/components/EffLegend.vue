<script lang="ts">
// The legend for the two warning colours of Eff, shared by every table that
// draws Eff bars so they read the same.
import { defineComponent } from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';

export default defineComponent({
  name: 'CostReportEffLegend',

  setup() {
    const { t } = useText(useStore());

    return { t };
  },
});
</script>

<template>
  <p class="cr-eff-legend">
    <span><i class="over" />{{ t('costReport.workloads.legendOver') }}</span>
    <span><i class="under" />{{ t('costReport.workloads.legendUnder') }}</span>
  </p>
</template>
