import { describe, it, expect } from '@jest/globals';
import { errorFrom, errorText, periodQuery, validDay } from '../api';

describe('errorFrom', () => {
  it('reads the backend code from a Status', () => {
    const e = errorFrom(429, {
      kind:    'Status',
      code:    429,
      message: 'm',
      details: { retryAfterSeconds: 12 },
      error:   {
        code: 'series_budget', message: 'retry in 12 s', retryAfterSeconds: 12
      }
    });

    expect([e.code, e.status, e.retryAfterSeconds, e.message]).toStrictEqual(['series_budget', 429, 12, 'retry in 12 s']);
    expect(errorFrom(400, { error: { code: 'bad_step', options: ['5m', 7, '1h'] } }).options).toStrictEqual(['5m', '1h']);
  });

  it('tells the API server’s own refusals from the backend’s', () => {
    expect(errorFrom(403, {
      kind: 'Status', reason: 'Forbidden', message: 'forbidden'
    }).code).toBe('forbidden');
    expect(errorFrom(403, { error: { code: 'no_department' } }).code).toBe('no_department');
    expect(errorFrom(503, null).code).toBe('unavailable');
    expect(errorFrom(500, { error: { code: 'something_new' } }).code).toBe('invalid');
    expect(errorFrom(429, { details: { retryAfterSeconds: 5 } }).retryAfterSeconds).toBe(5);
  });
});

describe('periodQuery', () => {
  it('a day wins over a window; extras are added', () => {
    expect(periodQuery({ window: '24h', day: '2026-09-24' })).toBe('?day=2026-09-24');
    expect(periodQuery({ window: '7d' }, { step: '1h' })).toBe('?window=7d&step=1h');
    expect(periodQuery({})).toBe('');
    expect(periodQuery({ window: '1h,2h' })).toBe('?window=1h%2C2h');
  });
});

describe('validDay', () => {
  it('accepts only a calendar day', () => {
    expect(validDay('2026-09-24')).toBe('2026-09-24');
    expect(validDay('2026-13-01')).toBeNull();
    expect(validDay('2026-9-1')).toBeNull();
    expect(validDay(['2026-09-24'])).toBeNull();
    expect(validDay(undefined)).toBeNull();
  });
});

describe('series errors', () => {
  const t = (k: string, a?: Record<string, unknown>) => (a ? `${ k }${ JSON.stringify(a) }` : k);

  it('known codes have their own text, unknown ones show the backend message', () => {
    const noSeg = errorFrom(502, { kind: 'Status', error: { code: 'no_segments', message: 'OpenCost returned no segments' } });

    expect(noSeg.code).toBe('no_segments');
    expect(errorText(t, noSeg)).toBe('costReport.errors.no_segments');
    for (const code of ['series_budget', 'bad_step', 'upstream']) {
      expect(errorFrom(400, {
        error: {
          code, message: 'x', retryAfterSeconds: 3
        }
      }).code).toBe(code);
    }
    expect(errorText(t, errorFrom(429, {
      error: {
        code: 'series_budget', message: 'x', retryAfterSeconds: 3
      }
    }))).toBe('costReport.errors.series_budget{"seconds":3}');
    const later = errorFrom(500, { error: { code: 'something_new', message: 'The backend explains it' } });

    expect(later.code).toBe('other');
    expect(errorText(t, later)).toBe('The backend explains it');
    // No body at all is still "not understood".
    expect(errorFrom(502, null).code).toBe('invalid');
  });
});
