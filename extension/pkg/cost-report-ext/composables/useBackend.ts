// The clusters running cost-report and the one shown, shared by both pages
// so switching pages does not search again.
import { ref, computed } from 'vue';
import {
  findCandidates, pickCandidate, rememberCluster, rememberedCluster, Candidate
} from '../utils/target';

const candidates = ref<Candidate[]>([]);
const selectedId = ref<string | null>(null);
const searching = ref(false);
const searched = ref(false);
let running: Promise<void> | null = null;

async function search() {
  searching.value = true;
  try {
    const found = await findCandidates();

    candidates.value = found;
    selectedId.value = pickCandidate(found, selectedId.value || rememberedCluster())?.clusterId || null;
    searched.value = true;
  } finally {
    searching.value = false;
  }
}

/** Forget what was found (tests). */
export function resetBackend() {
  candidates.value = [];
  selectedId.value = null;
  searched.value = false;
  running = null;
}

export function useBackend() {
  const selected = computed(() => candidates.value.find((c) => c.clusterId === selectedId.value) || null);

  return {
    candidates,
    selected,
    searching,
    searched,
    /** Find the clusters once (again with `force`). */
    ensure(force = false): Promise<void> {
      if (searched.value && !force) {
        return Promise.resolve();
      }
      if (!running) {
        running = search().finally(() => {
          running = null;
        });
      }

      return running;
    },
    select(clusterId: string) {
      selectedId.value = clusterId;
      rememberCluster(clusterId);
    },
  };
}
