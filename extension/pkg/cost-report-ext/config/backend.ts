// Where the page finds the cost-report backend, defined in one place.
// The contract is docs/api.md. Every request uses the signed-in user's own
// Rancher session; the extension keeps no token.

// The backend is a Kubernetes aggregated API on a Rancher-managed cluster:
// that cluster's API server verifies the user and checks RBAC before a
// request reaches it. All resources are cluster-scoped. Which clusters run
// it is found at run time (utils/target.ts).
export const API_GROUP = 'api.costreport.yansheng133.github.io';

export const API_VERSION = 'v1alpha1';

export const REQUEST_TIMEOUT_MS = 60 * 1000;

// The backend refreshes its cache every 60 s (ttlSeconds); the page asks
// again on the same beat while it is open.
export const REFRESH_INTERVAL_MS = 60 * 1000;

// The section being read is the last one whose top is above a line this
// far down the visible area below the sticky bar (at least SPY_MIN_PX).
export const SPY_FRACTION = 1 / 3;

export const SPY_MIN_PX = 40;

// Below this the coverage banner is shown (the backend's gate decides
// what the number means for billing; this only decides when to warn).
export const COVERAGE_WARN_PCT = 99.5;

// A sample shorter than two hours makes the 30-day estimate an
// extrapolation of more than 360 times: shown faded, with a warning.
export const THIN_SAMPLE_FACTOR = 360;

// Rows shown before "show the rest" in long tables.
export const TABLE_LIMITS = {
  workloads: 8, storage: 8, targets: 5, seals: 7
};
