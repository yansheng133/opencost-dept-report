<script lang="ts">
// A text with a ticking age ("updated 12 s ago"). It ticks on its own, so
// the page around it does not re-render every second (a page re-render
// reset the date field while someone was typing in it).
import {
  defineComponent, PropType, computed, ref, onMounted, onBeforeUnmount
} from 'vue';
import { useStore } from 'vuex';
import { useText } from '../composables/useText';

export default defineComponent({
  name: 'CostReportAgeText',

  props: {
    // Translation key; its arguments get `age` (seconds) added.
    textKey:     { type: String, required: true },
    args:        { type: Object as PropType<Record<string, unknown>>, default: () => ({}) },
    // When the data was made (ISO); without it `fallbackAge` is used as is.
    born:        { type: String as PropType<string | null>, default: null },
    fallbackAge: { type: Number, default: 0 },
  },

  setup(props) {
    const { t } = useText(useStore());
    const now = ref(Date.now());
    let timer: ReturnType<typeof setInterval> | null = null;

    onMounted(() => {
      timer = setInterval(() => {
        now.value = Date.now();
      }, 1000);
    });
    onBeforeUnmount(() => timer && clearInterval(timer));

    const text = computed(() => {
      const b = props.born ? Date.parse(props.born) : NaN;
      const age = isNaN(b) ? props.fallbackAge : Math.max(0, Math.round((now.value - b) / 1000));

      return t(props.textKey, { ...props.args, age });
    });

    return { text };
  },
});
</script>

<template>
  <span>{{ text }}</span>
</template>
