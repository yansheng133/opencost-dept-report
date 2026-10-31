// HTTP client for the cost-report backend, through Rancher's
// /k8s/clusters/<id> proxy with the user's own Rancher session (the browser
// sends the session cookie). The API is read-only, so no CSRF token is
// needed. Errors are Kubernetes `Status` objects; the backend adds
// `error: { code, message, ... }` (docs/api.md).
import { REQUEST_TIMEOUT_MS } from '../config/backend';
import { apiBase } from './target';
import type {
  BackendConfig, Report, Series, SealRow, Month, Scope
} from './types';

export type ErrorCode =
  | 'bad_window' | 'bad_step' | 'no_department' | 'not_found' | 'series_budget'
  | 'upstream' | 'no_segments' | 'not_ready' | 'unauthenticated'
  // A code this page does not know: its text is the backend's message.
  | 'other'
  // Not from the backend: the API server refused (RBAC), the request did
  // not get through, or the answer was not understood.
  | 'forbidden' | 'unreachable' | 'unavailable' | 'invalid';

export class ApiError extends Error {
  status: number;
  code: ErrorCode;
  retryAfterSeconds?: number;
  options?: string[];

  constructor(status: number, code: ErrorCode, message: string, extra: { retryAfterSeconds?: number; options?: string[] } = {}) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
    this.retryAfterSeconds = extra.retryAfterSeconds;
    this.options = extra.options;
  }
}

const KNOWN: ErrorCode[] = ['bad_window', 'bad_step', 'no_department', 'not_found', 'series_budget', 'upstream', 'no_segments', 'not_ready', 'unauthenticated'];

/**
 * The text of an error: the page's own for known codes, the backend's
 * message for codes it does not know.
 */
export function errorText(t: (k: string, a?: Record<string, unknown>) => string, e: ApiError): string {
  if (e.code === 'other') {
    return e.message;
  }
  if (e.code === 'series_budget') {
    return t('costReport.errors.series_budget', { seconds: e.retryAfterSeconds ?? '?' });
  }

  return t(`costReport.errors.${ e.code }`);
}

/** The error an unsuccessful answer stands for. */
export function errorFrom(status: number, body: unknown): ApiError {
  const b = (body || {}) as {
    message?: unknown;
    details?: { retryAfterSeconds?: unknown };
    error?: { code?: unknown; message?: unknown; retryAfterSeconds?: unknown; options?: unknown };
  };
  const e = b.error || {};
  const message = typeof e.message === 'string' ? e.message : (typeof b.message === 'string' ? b.message : `HTTP ${ status }`);
  const retry = typeof e.retryAfterSeconds === 'number' ? e.retryAfterSeconds : (typeof b.details?.retryAfterSeconds === 'number' ? b.details.retryAfterSeconds : undefined);
  const options = Array.isArray(e.options) ? e.options.filter((o): o is string => typeof o === 'string') : undefined;
  let code: ErrorCode;

  if (typeof e.code === 'string' && (KNOWN as string[]).includes(e.code)) {
    code = e.code as ErrorCode;
  } else if (typeof e.code === 'string' && e.code && typeof e.message === 'string' && e.message) {
    // A newer backend's code: show what it says rather than "not understood".
    code = 'other';
  } else if (status === 403) {
    code = 'forbidden';
  } else if (status === 503) {
    code = 'unavailable';
  } else if (status === 0) {
    code = 'unreachable';
  } else {
    code = 'invalid';
  }

  return new ApiError(status, code, message, { retryAfterSeconds: retry, options });
}

/** Query string for a period: one day, else a window (else the default). */
export function periodQuery(view: { window?: string | null; day?: string | null }, extra: Record<string, string> = {}): string {
  const q = new URLSearchParams();

  if (view.day) {
    q.set('day', view.day);
  } else if (view.window) {
    q.set('window', view.window);
  }
  Object.entries(extra).forEach(([k, v]) => q.set(k, v));
  const s = q.toString();

  return s ? `?${ s }` : '';
}

const DAY = /^\d{4}-\d{2}-\d{2}$/;

/** A day from the address bar, if it is one (YYYY-MM-DD). */
export function validDay(v: unknown): string | null {
  return typeof v === 'string' && DAY.test(v) && !isNaN(Date.parse(`${ v }T00:00:00Z`)) ? v : null;
}

async function get<T>(clusterId: string, path: string): Promise<T> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  let res: Response;

  try {
    res = await fetch(`${ apiBase(clusterId) }/${ path }`, {
      headers: { Accept: 'application/json' }, credentials: 'same-origin', cache: 'no-store', signal: controller.signal
    });
  } catch {
    throw new ApiError(0, 'unreachable', 'request did not get through');
  } finally {
    clearTimeout(timer);
  }
  let body: unknown = null;

  try {
    body = await res.json();
  } catch {}
  if (!res.ok) {
    throw errorFrom(res.status, body);
  }
  if (!body || typeof body !== 'object') {
    throw new ApiError(res.status, 'invalid', 'not a JSON object');
  }

  return body as T;
}

export const getConfig = (c: string) => get<BackendConfig>(c, 'config');

export const getReport = (c: string, view: { window?: string | null; day?: string | null }) => get<Report>(c, `reports${ periodQuery(view) }`);

export const getSeries = (c: string, view: { window?: string | null; day?: string | null }, step: string) => get<Series>(c, `series${ periodQuery(view, { step }) }`);

export const getSeals = (c: string) => get<{ enabled: boolean; scope: Scope; seals: SealRow[] }>(c, 'seals');

export const getPeriods = (c: string) => get<{ scope?: Scope; months: Month[] }>(c, 'periods');
