// Which Rancher-managed clusters run cost-report, and which one the page
// shows. Each cluster that runs OpenCost can run its own cost-report, so
// there may be several; the page offers a choice when there are.
//
// 1. Every cluster the user can see is asked for the API group's discovery
//    document (any signed-in user may read it). 200 means installed.
// 2. Those are asked for `config`. 200: ready. 403 with error.code
//    no_department: installed, but the user may see no department there.
//    Any other 403 (the API server's RBAC): installed, no access. 503:
//    installed, not answering yet (restarting, waiting for OpenCost).
// 3. The choice made last time (this browser) is kept if that cluster is
//    still among them.
import { API_GROUP, API_VERSION } from '../config/backend';
import type { BackendConfig } from './types';

export interface ProbeAnswer {
  status: number;
  body: unknown;
}

// One GET; status 0 when it did not get through.
export type Probe = (url: string) => Promise<ProbeAnswer>;

export type Access = 'ready' | 'no_department' | 'forbidden' | 'unavailable';

export interface Candidate {
  clusterId: string;
  // Rancher's display name, else the ID.
  name: string;
  access: Access;
  config: BackendConfig | null;
}

const STORAGE_KEY = 'cost-report-cluster';
const PROBE_TIMEOUT_MS = 8000;

export function apiBase(clusterId: string): string {
  return `/k8s/clusters/${ encodeURIComponent(clusterId) }/apis/${ API_GROUP }/${ API_VERSION }`;
}

export const fetchProbe: Probe = async(url) => {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), PROBE_TIMEOUT_MS);

  try {
    const res = await fetch(url, {
      headers: { Accept: 'application/json' }, credentials: 'same-origin', signal: controller.signal
    });
    let body: unknown = null;

    try {
      body = await res.json();
    } catch {}

    return { status: res.status, body };
  } catch {
    return { status: 0, body: null };
  } finally {
    clearTimeout(timer);
  }
};

/** What an answer to GET .../config says about access. */
export function accessFrom(a: ProbeAnswer): Access {
  if (a.status === 200) {
    // A caller without any department gets config with no departments;
    // every other resource answers 403 no_department.
    const c = a.body as Partial<BackendConfig> | null;

    return !c?.scope?.all && !(c?.departments || []).length ? 'no_department' : 'ready';
  }
  if (a.status === 403) {
    const code = (a.body as { error?: { code?: unknown } } | null)?.error?.code;

    return code === 'no_department' ? 'no_department' : 'forbidden';
  }

  return 'unavailable';
}

interface ClusterItem { id?: unknown; spec?: { displayName?: unknown } }

/** Every cluster with cost-report, in Rancher's list order. */
export async function findCandidates(probe: Probe = fetchProbe): Promise<Candidate[]> {
  const list = await probe('/v1/management.cattle.io.clusters');
  const clusters = ((list.body as { data?: ClusterItem[] } | null)?.data || [])
    .filter((c): c is ClusterItem & { id: string } => typeof c.id === 'string' && !!c.id)
    .map((c) => ({ id: c.id, name: typeof c.spec?.displayName === 'string' && c.spec.displayName ? c.spec.displayName : c.id }));
  const found = await Promise.all(clusters.map(async(c): Promise<Candidate | null> => {
    const discovery = await probe(apiBase(c.id));

    if (discovery.status !== 200) {
      return null;
    }
    const a = await probe(`${ apiBase(c.id) }/config`);
    const access = accessFrom(a);

    return {
      clusterId: c.id,
      name:      c.name,
      access,
      config:    a.status === 200 ? a.body as BackendConfig : null,
    };
  }));

  return found.filter((c): c is Candidate => !!c);
}

export function rememberedCluster(): string | null {
  try {
    return window.localStorage.getItem(STORAGE_KEY) || null;
  } catch {
    return null;
  }
}

export function rememberCluster(clusterId: string) {
  try {
    window.localStorage.setItem(STORAGE_KEY, clusterId);
  } catch {}
}

/**
 * The cluster to show: the remembered one if still a candidate, else the
 * first the user can read, else the first at all (to explain why not).
 */
export function pickCandidate(candidates: Candidate[], remembered: string | null): Candidate | null {
  return candidates.find((c) => c.clusterId === remembered) ||
    candidates.find((c) => c.access === 'ready') ||
    candidates[0] ||
    null;
}
