// What the caller may see (Report.scope) and the department filter on the
// page. The backend already leaves out what is outside the scope; these
// helpers decide what the page shows, and never show a cluster-wide
// figure that is null as if it were zero.
import type {
  Department, Report, Scope, Workload, StorageRow, EnvRow, Month
} from './types';

// Colors assigned by the department's position in config.departments, so a
// department keeps its color across periods and pages. Readable on both
// Rancher themes.
export const PALETTE = ['#3d8fd6', '#e8743b', '#1baf7a', '#b067c9', '#d4a72c', '#4bb3c4', '#d9546a', '#7c8bd9'];

// Idle and unallocated are not departments.
export const IDLE_COLOR = 'var(--muted, #8b8e95)';

export const UNALLOCATED_COLOR = 'var(--darker, #6e7076)';

export interface DeptIndex {
  name: (id: string) => string;
  color: (id: string | null | undefined) => string;
}

/** Names and colors from config.departments; unknown IDs show as they are. */
export function deptIndex(departments: Department[]): DeptIndex {
  const names = new Map(departments.map((d) => [d.id, d.name || d.id]));
  const order = new Map(departments.map((d, i) => [d.id, i]));
  const extra = new Map<string, number>();

  return {
    name:  (id) => names.get(id) || id,
    color: (id) => {
      if (!id) {
        return UNALLOCATED_COLOR;
      }
      let i = order.get(id);

      if (i === undefined) {
        // A department the config does not list (labels added later):
        // colors after the listed ones, stable while the page is open.
        i = extra.get(id);
        if (i === undefined) {
          i = departments.length + extra.size;
          extra.set(id, i);
        }
      }

      return PALETTE[i % PALETTE.length];
    },
  };
}

export const isAll = (s: Scope | null | undefined) => !!s?.all;

/** Which cluster-wide parts the page may show. */
export function visibleParts(r: Report) {
  const all = isAll(r.scope);

  return {
    idle:           all && r.idle !== null && r.idle !== undefined,
    unallocated:    all && !!r.unallocated,
    reconciliation: all && !!r.reconciliation,
    targets:        all && Array.isArray(r.targets),
    unallocRows:    all && Array.isArray(r.unallocRows),
    fill:           all && !!r.basis?.fill,
  };
}

// The department filter: 'all' or one department ID.
export type DeptFilter = string;

export const inFilter = (f: DeptFilter, id: string | null | undefined) => f === 'all' || id === f;

export function filterWorkloads(list: Workload[], f: DeptFilter): Workload[] {
  return list.filter((w) => inFilter(f, w.dept));
}

export function filterStorage(rows: StorageRow[], f: DeptFilter): StorageRow[] {
  return rows.filter((r) => f === 'all' || r.depts.some((d) => d.id === f));
}

export function filterEnv(rows: EnvRow[], f: DeptFilter): EnvRow[] {
  return rows.filter((r) => inFilter(f, r.dept));
}

/** The filter, unless the department is no longer in the report. */
export function validFilter(f: DeptFilter, r: Report): DeptFilter {
  return f === 'all' || r.depts.some((d) => d.id === f) ? f : 'all';
}

/** Department amounts of a month, largest first, inside the scope. */
export function monthDepts(m: Month): { id: string; total: number }[] {
  return Object.entries(m.depts || {})
    .map(([id, total]) => ({ id, total }))
    .sort((a, b) => b.total - a.total);
}
