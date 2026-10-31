<script lang="ts">
// A label with an explanation that shows on hover AND on keyboard focus
// (hover alone leaves touch and keyboard users without it). The
// explanation is placed on the label, not on the number next to it.
import { defineComponent, ref, nextTick } from 'vue';

export default defineComponent({
  name: 'CostReportHelp',

  props: {
    text: { type: String, required: true },
    tip:  { type: String, default: '' },
  },

  setup() {
    const el = ref<HTMLElement | null>(null);
    const box = ref<HTMLElement | null>(null);
    const shown = ref(false);
    const pos = ref({ left: '0px', top: '0px' });

    async function show() {
      shown.value = true;
      await nextTick();
      const r = el.value?.getBoundingClientRect();
      const w = box.value?.offsetWidth || 0;
      const h = box.value?.offsetHeight || 0;

      if (r) {
        pos.value = {
          left: `${ Math.min(Math.max(r.left, 8), window.innerWidth - w - 8) }px`,
          top:  `${ r.top - h - 10 < 8 ? r.bottom + 10 : r.top - h - 10 }px`,
        };
      }
    }

    return {
      el,
      box,
      shown,
      pos,
      show,
      hide: () => {
        shown.value = false;
      }
    };
  },
});
</script>

<template>
  <span class="cr-help-wrap">
    <span
      v-if="tip"
      ref="el"
      class="cr-help"
      tabindex="0"
      role="note"
      :aria-label="`${ text }: ${ tip }`"
      @pointerenter="show"
      @pointerleave="hide"
      @focus="show"
      @blur="hide"
    >{{ text }}</span>
    <span v-else>{{ text }}</span>
    <Teleport
      v-if="tip"
      to="body"
    >
      <span
        v-show="shown"
        ref="box"
        class="cr-help-tip"
        role="tooltip"
        :style="pos"
      >{{ tip }}</span>
    </Teleport>
  </span>
</template>

<style lang="scss">
.cr-help {
  border-bottom: 1px dotted var(--muted, #888);
  cursor: help;

  &:focus-visible {
    outline: 2px solid var(--primary, #3d8fd6);
    outline-offset: 2px;
  }
}

.cr-help-tip {
  position: fixed;
  z-index: 1000;
  max-width: 320px;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: var(--border-radius, 4px);
  background: var(--body-bg);
  color: var(--body-text);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.18);
  font-size: 12.5px;
  font-weight: normal;
  line-height: 1.6;
  text-align: left;
  white-space: normal;
  pointer-events: none;
}
</style>
