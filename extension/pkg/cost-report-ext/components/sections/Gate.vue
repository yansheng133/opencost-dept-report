<script lang="ts">
// 能不能出帳: the backend's billing checks, in order, with the policy
// thresholds it used.
import { defineComponent, PropType, computed } from 'vue';
import { useReportContext } from '../../composables/useReportContext';
import { gateCheckText } from '../../utils/describe';
import { n2 } from '../../utils/format';
import type { Gate } from '../../utils/types';

export default defineComponent({
  name: 'CostReportGate',

  props: { gate: { type: Object as PropType<Gate>, required: true } },

  setup(props) {
    const ctx = useReportContext();
    const checks = computed(() => props.gate.checks.map((c) => ({ ...c, ...gateCheckText(ctx.value.t, c, props.gate.thresholds || {}) })));

    return {
      ctx, checks, n2
    };
  },
});
</script>

<template>
  <section
    id="gate"
    class="cr-gate"
    :class="gate.verdict"
  >
    <div class="cr-gate-head">
      <h2>{{ ctx.t('costReport.gate.title') }}</h2>
      <span
        class="cr-tag"
        :class="gate.verdict"
      >{{ ctx.t(`costReport.gate.verdict.${ gate.verdict }`) }}</span>
    </div>
    <p class="cr-desc">
      {{ ctx.t(`costReport.gate.verdictText.${ gate.verdict }`) }}
    </p>
    <div class="cr-checks">
      <div
        v-for="c in checks"
        :key="c.code"
        class="cr-check"
      >
        <span
          class="cr-dot"
          :class="c.status"
          :title="ctx.t(`costReport.gate.status.${ c.status }`)"
        />
        <div>
          <div class="cr-check-name">
            {{ c.name }}
            <span class="text-muted cr-small">· {{ ctx.t(`costReport.gate.status.${ c.status }`) }}</span>
          </div>
          <div
            v-if="c.detail"
            class="cr-small"
          >
            {{ c.detail }}
          </div>
          <div class="text-muted cr-small">
            {{ c.why }}
          </div>
          <div class="text-muted cr-small">
            {{ ctx.t('costReport.gate.policy', { text: c.policy }) }}
          </div>
        </div>
        <div class="cr-num cr-check-val">
          {{ c.value === null || c.value === undefined ? '—' : `${ n2(c.value) }${ c.unit || '' }` }}
        </div>
      </div>
    </div>
  </section>
</template>
