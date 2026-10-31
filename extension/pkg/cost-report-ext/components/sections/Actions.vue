<script lang="ts">
// 待處理事項, computed from this period's data (utils/actions.ts).
import { defineComponent, PropType, computed } from 'vue';
import CrSection from '../CrSection.vue';
import { useReportContext } from '../../composables/useReportContext';
import { nextActions } from '../../utils/actions';
import { actionText } from '../../utils/describe';
import type { Report } from '../../utils/types';

export default defineComponent({
  name: 'CostReportActions',

  components: { CrSection },

  props: {
    report:     { type: Object as PropType<Report>, required: true },
    idleShared: { type: Boolean, required: true },
    month:      { type: Number, required: true },
  },

  setup(props) {
    const ctx = useReportContext();
    const items = computed(() => {
      const c = ctx.value;

      return nextActions(props.report, props.idleShared).map((a) => actionText({
        t: c.t, name: c.depts.name, label: c.label, month: props.month
      }, a));
    });

    return { ctx, items };
  },
});
</script>

<template>
  <CrSection
    id="next"
    :title="ctx.t('costReport.actions.title')"
    :description="ctx.t('costReport.actions.description')"
  >
    <ol class="cr-actions">
      <li
        v-for="a in items"
        :key="a.title"
      >
        <b>{{ a.title }}</b>
        <div class="text-muted">
          {{ a.text }}
        </div>
      </li>
    </ol>
  </CrSection>
</template>
