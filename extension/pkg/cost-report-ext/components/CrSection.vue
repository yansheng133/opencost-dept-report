<script lang="ts">
// One section of a page: a heading that folds the section, an anchor link,
// a short description and the content. Folded sections are remembered in
// this browser (a convenience: failing storage changes nothing else).
import { defineComponent, ref, watch } from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';

const STORAGE_KEY = 'cost-report-folded';

export function foldedSet(): Set<string> {
  try {
    return new Set(JSON.parse(window.localStorage.getItem(STORAGE_KEY) || '[]'));
  } catch {
    return new Set();
  }
}

export function saveFolded(id: string, on: boolean) {
  const s = foldedSet();

  if (on) {
    s.add(id);
  } else {
    s.delete(id);
  }
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify([...s]));
  } catch {}
}

// Folding all sections at once (the page's buttons) reaches every mounted
// section through this.
export const foldAll = ref<{ on: boolean; at: number } | null>(null);

// The department the page is filtered to ('all' when none). Sections
// about the whole cluster are not filtered, and say so while a filter is on.
export const activeFilter = ref('all');

export default defineComponent({
  name: 'CrSection',

  props: {
    id:          { type: String, required: true },
    title:       { type: String, required: true },
    description: { type: String, default: '' },
    // Shown next to the title while folded, so a folded page still reads.
    summary:     { type: String, default: '' },
    // The section is about the whole cluster, not the filtered department.
    clusterWide: { type: Boolean, default: false },
  },

  setup(props) {
    const store = useStore();
    const { t } = useText(store);
    const folded = ref(foldedSet().has(props.id));

    function toggle(on = !folded.value) {
      folded.value = on;
      saveFolded(props.id, on);
    }
    watch(foldAll, (v) => v && toggle(v.on));

    return {
      t, folded, toggle, activeFilter
    };
  },
});
</script>

<template>
  <section
    :id="id"
    class="cr-section"
    :class="{ folded }"
  >
    <div class="cr-section-head">
      <button
        type="button"
        class="btn btn-sm role-link cr-fold"
        :aria-expanded="!folded"
        :title="folded ? t('costReport.report.unfold') : t('costReport.report.fold')"
        @click="toggle()"
      >
        <i :class="folded ? 'icon icon-chevron-right' : 'icon icon-chevron-down'" />
      </button>
      <h2>{{ title }}</h2>
      <span
        v-if="clusterWide && activeFilter !== 'all'"
        class="cr-tag cr-tag-scope"
        :title="t('costReport.report.clusterWideTip')"
      >{{ t('costReport.report.clusterWide') }}</span>
      <span
        v-if="folded && summary"
        class="text-muted cr-fold-summary"
      >{{ summary }}</span>
      <a
        class="cr-anchor"
        :href="`#${ id }`"
      >#</a>
    </div>
    <div v-show="!folded">
      <p
        v-if="description"
        class="text-muted cr-desc"
      >
        {{ description }}
      </p>
      <slot />
    </div>
  </section>
</template>
