<script lang="ts">
// Which cluster's cost-report the page shows. A selector appears only when
// more than one cluster runs it; the states where nothing can be shown
// (none installed, no access, not answering) are explained here.
import { defineComponent, computed } from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';
import { useBackend } from '../composables/useBackend';

export default defineComponent({
  name: 'CostReportTargetBar',

  setup() {
    const { t } = useText(useStore());
    const b = useBackend();
    const message = computed(() => {
      if (!b.searched.value) {
        return '';
      }
      const s = b.selected.value;

      if (!s) {
        return t('costReport.target.none');
      }

      return {
        ready: '', no_department: t('costReport.target.noDepartment'), forbidden: t('costReport.target.forbidden'), unavailable: t('costReport.target.unavailable')
      }[s.access];
    });

    return {
      t,
      candidates: b.candidates,
      selected:   b.selected,
      searching:  b.searching,
      message,
      choose:     (e: Event) => b.select((e.target as HTMLSelectElement).value),
      retry:      () => b.ensure(true),
    };
  },
});
</script>

<template>
  <div class="cr-target">
    <label
      v-if="candidates.length > 1"
      class="cr-target-pick"
    >
      <span>{{ t('costReport.target.cluster') }}</span>
      <select
        :value="selected && selected.clusterId"
        @change="choose"
      >
        <option
          v-for="c in candidates"
          :key="c.clusterId"
          :value="c.clusterId"
        >
          {{ c.name }} {{ t(`costReport.target.access.${ c.access }`) }}
        </option>
      </select>
    </label>
    <p
      v-if="searching"
      class="text-muted"
    >
      {{ t('costReport.target.searching') }}
    </p>
    <div
      v-else-if="message"
      class="cr-banner cr-banner-info"
    >
      {{ message }}
      <button
        type="button"
        class="btn btn-sm role-secondary ml-10"
        @click="retry"
      >
        {{ t('costReport.common.refresh') }}
      </button>
    </div>
  </div>
</template>
