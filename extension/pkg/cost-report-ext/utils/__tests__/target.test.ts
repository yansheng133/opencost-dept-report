import { describe, it, expect } from '@jest/globals';
import {
  findCandidates, pickCandidate, accessFrom, apiBase, Probe, ProbeAnswer
} from '../target';

const GV = '/apis/api.costreport.yansheng133.github.io/v1alpha1';

// A Rancher with these clusters; `installed` maps the clusters running
// cost-report to what GET config answers there.
function rancher(clusters: { id: string; displayName?: string }[], installed: Record<string, ProbeAnswer>) {
  const asked: string[] = [];
  const probe: Probe = async(url) => {
    asked.push(url);
    if (url === '/v1/management.cattle.io.clusters') {
      return { status: 200, body: { data: clusters.map((c) => ({ id: c.id, spec: { displayName: c.displayName } })) } };
    }
    const m = /^\/k8s\/clusters\/([^/]+)(.*)$/.exec(url);

    if (!m) {
      return { status: 404, body: null };
    }
    const [, id, rest] = m;
    const here = installed[id];

    if (rest === GV) {
      return { status: here ? 200 : 404, body: {} };
    }
    if (rest === `${ GV }/config` && here) {
      return here;
    }

    return { status: 404, body: null };
  };

  return { probe, asked };
}

const config = (cluster: string) => ({
  status: 200,
  body:   {
    cluster, windows: ['1h', '24h'], defaultWindow: '24h', departments: [], scope: { all: true, departments: [] }
  }
});

describe('findCandidates', () => {
  it('lists every cluster with the API group, with its access', async() => {
    const r = rancher([{ id: 'local' }, { id: 'c-1', displayName: 'prod' }, { id: 'c-2' }, { id: 'c-3' }], {
      'c-1': config('prod'),
      'c-2': { status: 403, body: { kind: 'Status', error: { code: 'no_department' } } },
      'c-3': { status: 403, body: { kind: 'Status', reason: 'Forbidden' } },
    });
    const found = await findCandidates(r.probe);

    expect(found.map((c) => [c.clusterId, c.name, c.access])).toStrictEqual([
      ['c-1', 'prod', 'ready'], ['c-2', 'c-2', 'no_department'], ['c-3', 'c-3', 'forbidden']
    ]);
    expect(found[0].config?.cluster).toBe('prod');
    expect(found[1].config).toBeNull();
    expect(found[2].config).toBeNull();
    // config is asked only where the group exists
    expect(r.asked).not.toContain(`/k8s/clusters/local${ GV }/config`);
  });

  it('finds nothing when no cluster has it, or the list cannot be read', async() => {
    expect(await findCandidates(rancher([{ id: 'local' }], {}).probe)).toStrictEqual([]);
    expect(await findCandidates(async() => ({ status: 0, body: null }))).toStrictEqual([]);
  });

  it('a backend restarting is installed but unavailable', async() => {
    const found = await findCandidates(rancher([{ id: 'c-1' }], { 'c-1': { status: 503, body: {} } }).probe);

    expect(found[0].access).toBe('unavailable');
  });
});

describe('pickCandidate', () => {
  const c = (clusterId: string, access: 'ready' | 'forbidden') => ({
    clusterId, name: clusterId, access, config: null
  });

  it('keeps the remembered cluster, else the first readable one', () => {
    const list = [c('a', 'forbidden'), c('b', 'ready'), c('c', 'ready')];

    expect(pickCandidate(list, 'c')?.clusterId).toBe('c');
    expect(pickCandidate(list, 'gone')?.clusterId).toBe('b');
    expect(pickCandidate(list, null)?.clusterId).toBe('b');
    expect(pickCandidate([c('a', 'forbidden')], null)?.clusterId).toBe('a');
    expect(pickCandidate([], 'a')).toBeNull();
  });
});

describe('accessFrom and apiBase', () => {
  it('reads the answer', () => {
    expect(accessFrom({ status: 200, body: { scope: { all: false, departments: ['rd'] }, departments: [{ id: 'rd' }] } })).toBe('ready');
    expect(accessFrom({ status: 200, body: { scope: { all: true, departments: [] }, departments: [] } })).toBe('ready');
    // no department: config answers 200 with none
    expect(accessFrom({ status: 200, body: { scope: { all: false, departments: [] }, departments: [] } })).toBe('no_department');
    expect(accessFrom({ status: 403, body: { error: { code: 'no_department' } } })).toBe('no_department');
    expect(accessFrom({ status: 403, body: null })).toBe('forbidden');
    expect(accessFrom({ status: 0, body: null })).toBe('unavailable');
    expect(apiBase('c-m/1')).toBe(`/k8s/clusters/c-m%2F1${ GV }`);
  });
});
