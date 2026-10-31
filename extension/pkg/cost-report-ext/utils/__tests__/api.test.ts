import { describe, it, expect } from '@jest/globals';
import { errorFrom, periodQuery, validDay } from '../api';

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
