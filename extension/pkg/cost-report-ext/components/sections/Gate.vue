<script lang="ts">
// 能不能出帳: the backend's billing checks, in order, with the policy
// thresholds it used. One line (verdict and how many checks did not pass)
// is always shown; the checks fold like any section. They start folded
// only when everything passed: a reader who comes for the amounts should
// not have to scroll past six passing checks, but must see failing ones.
import {
  defineComponent, PropType, computed, ref, watch
} from 'vue';
import { useReportContext } from '../../composables/useReportContext';
import {
  foldAll, foldedSet, saveFolded, activeFilter, openRequest
} from '../CrSection.vue';
import { gateCheckText, gateSummary, gateVerdictText } from '../../utils/describe';
import { n2, pctValue } from '../../utils/format';
import type { Gate } from '../../utils/types';

const ID = 'gate';

export default defineComponent({
  name: 'CostReportGate',

  props: { gate: { type: Object as PropType<Gate>, required: true } },

  setup(props) {
    const ctx = useReportContext();
    const checks = computed(() => props.gate.checks.map((c) => ({ ...c, ...gateCheckText(ctx.value.t, c, props.gate.thresholds || {}) })));
    const folded = ref(foldedSet().has(ID) || props.gate.verdict === 'ok');

    function toggle(on = !folded.value) {
      folded.value = on;
      saveFolded(ID, on);
    }
    watch(foldAll, (v) => v && toggle(v.on));
    watch(openRequest, (v) => v && v.id === ID && folded.value && toggle(false));
    // A verdict that gets worse opens the checks again.
    watch(() => props.gate.verdict, (v, old) => {
      if (v !== 'ok' && old === 'ok') {
        folded.value = false;
      }
    });

    return {
      ctx, checks, n2, pctValue, folded, toggle, activeFilter, summary: computed(() => gateSummary(ctx.value.t, props.gate)), because: computed(() => gateVerdictText(ctx.value.t, props.gate))
    };
  },
});
</script>

<template>
  <section
    id="gate"
    class="cr-gate"
    :class="[gate.verdict, { folded }]"
  >
    <div class="cr-gate-head">
      <button
        type="button"
        class="btn btn-sm role-link cr-fold"
        :aria-expanded="!folded"
        :title="folded ? ctx.t('costReport.report.unfold') : ctx.t('costReport.report.fold')"
        @click="toggle()"
      >
        <i :class="folded ? 'icon icon-chevron-right' : 'icon icon-chevron-down'" />
      </button>
      <h2>{{ ctx.t('costReport.gate.title') }}</h2>
      <span
        class="cr-tag"
        :class="gate.verdict"
      >{{ ctx.t(`costReport.gate.verdict.${ gate.verdict }`) }}</span>
      <span class="cr-gate-summary">{{ summary }}</span>
      <span
        v-if="activeFilter !== 'all'"
        class="cr-tag cr-tag-scope"
        :title="ctx.t('costReport.report.clusterWideTip')"
      >{{ ctx.t('costReport.report.clusterWide') }}</span>
    </div>
    <div v-show="!folded">
      <p class="cr-desc">
        {{ ctx.t(`costReport.gate.verdictText.${ gate.verdict }`) }} {{ because }}
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
              class="text-muted cr-small"
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
            {{ c.value === null || c.value === undefined || c.status === 'unknown' ? '—' : (c.unit === '%' ? pctValue(c.value) : `${ n2(c.value) }${ c.unit || '' }`) }}
          </div>
        </div>
      </div>
    </div>
  </section>
</template>
