import { describe, it, expect } from '@jest/globals';
import {
  averages, buildChart, partialIndex, spreadLabels, timedPoints, valueOf, wantedStep, nearestPoint, xOf, BOX
} from '../series';
import type { Series, SeriesPoint } from '../types';

const T0 = Date.parse('2026-09-24T00:00:00Z');
const at = (min: number) => new Date(T0 + min * 60000).toISOString();
const pt = (min: number, rd: number | null, minutes = 10): SeriesPoint => ({
  start: at(min), minutes, depts: rd === null ? null : { rd: { cpu: rd, ram: rd / 2 } }
});
const series = (points: SeriesPoint[], step = 10): Series => ({
  step: `${ step }m`, stepMinutes: step, points, covered: points.filter((p) => p.depts).length, expected: points.length, options: ['5m', '10m'], defaultStep: '10m'
});

describe('buildChart', () => {
  it('breaks the line where there is no data, and places points by time', () => {
    const s = series([pt(0, 0.5), pt(10, 0.6), pt(20, null), pt(30, 0.4), pt(40, 0.4)]);
    const c = buildChart(s, ['rd'], 'cpu', T0 + 1e9)!;

    expect(c.lines[0].segments.map((seg) => seg.length)).toStrictEqual([2, 2]);
    // even time spacing → even x spacing, including across the gap
    const xs = c.lines[0].segments.flat().map(([x]) => x);

    expect(xs[1] - xs[0]).toBeCloseTo((xs[3] - xs[2]));
    expect(xs[2] - xs[1]).toBeCloseTo(2 * (xs[1] - xs[0]));
    expect(c.maxY).toBe(60);
  });

  it('does not stretch the axis over skipped segments', () => {
    // a gap of 3 days between two points must be 3 days on the axis
    const s = series([pt(0, 0.5), pt(10, 0.5), pt(4330, 0.5)]);
    const c = buildChart(s, ['rd'], 'cpu', T0 + 1e10)!;
    const xs = c.lines[0].segments.flat().map(([x]) => x);

    expect((xs[1] - xs[0]) / (xs[2] - xs[0])).toBeCloseTo(10 / 4330);
    expect(c.xTicks.every((t) => t.withDate)).toBe(true);
  });

  it('a point alone stays a point; missing departments are null', () => {
    const s = series([pt(0, null), pt(10, 0.3), pt(20, null)]);
    const c = buildChart(s, ['rd', 'it'], 'ram', T0 + 1e9)!;

    expect(c.lines[0].segments).toStrictEqual([[[expect.any(Number), expect.any(Number)]]]);
    expect(c.lines[1].segments).toStrictEqual([]);
    expect(valueOf(s.points[1], 'it', 'cpu')).toBeNull();
    expect(valueOf(s.points[1], 'rd', 'ram')).toBeCloseTo(15);
    expect(c.maxY).toBe(20);
  });

  it('the unfinished last segment becomes the dashed tail', () => {
    const now = T0 + 25 * 60000;
    const s = series([pt(0, 0.5), pt(10, 0.5), pt(20, 0.2, 5)]);
    const c = buildChart(s, ['rd'], 'cpu', now)!;

    expect(c.partialIdx).toBe(2);
    expect(c.lines[0].segments[0]).toHaveLength(2);
    expect(c.lines[0].tail?.from).toStrictEqual(c.lines[0].segments[0][1]);
  });

  it('nothing to draw without timed points', () => {
    expect(buildChart(series([{ start: '', depts: null }]), ['rd'], 'cpu')).toBeNull();
    expect(timedPoints(null)).toStrictEqual([]);
  });
});

describe('partialIndex', () => {
  const pts = (minutes: number) => timedPoints(series([pt(0, 0.1), pt(10, 0.1, minutes)]));

  it('only a short last segment that has not ended', () => {
    expect(partialIndex(pts(5), 10, T0 + 15 * 60000)).toBe(1);
    // a past day: short last segment is missing data, not unfinished
    expect(partialIndex(pts(5), 10, T0 + 1e9)).toBe(-1);
    expect(partialIndex(pts(10), 10, T0 + 15 * 60000)).toBe(-1);
  });
});

describe('spreadLabels', () => {
  it('pushes overlapping labels apart and keeps them in the plot', () => {
    const out = spreadLabels([{ id: 'a', y: 100 }, { id: 'b', y: 100 }, { id: 'c', y: 101 }]);

    expect(out.map((l) => l.y)).toStrictEqual([100, 118, 136]);
    const low = spreadLabels([{ id: 'a', y: BOX.H - BOX.B }, { id: 'b', y: BOX.H - BOX.B }]);

    expect(low.map((l) => l.y)).toStrictEqual([BOX.H - BOX.B - 18, BOX.H - BOX.B]);
  });
});

describe('wantedStep and nearestPoint', () => {
  it('asks for the chosen step only when offered and different', () => {
    const s = series([pt(0, 0.1)]);

    expect(wantedStep('5m', s)).toBe('5m');
    expect(wantedStep('10m', s)).toBeNull();
    expect(wantedStep('1d', s)).toBeNull();
    expect(wantedStep(null, s)).toBeNull();
  });

  it('finds the nearest point', () => {
    const c = buildChart(series([pt(0, 0.1), pt(10, 0.1), pt(20, 0.1)]), ['rd'], 'cpu', T0 + 1e9)!;

    expect(nearestPoint(c, xOf(c.pts[1].ts, c.t0, c.t1) + 1)).toBe(1);
  });
});

describe('averages', () => {
  it('weights by minutes and skips segments without data', () => {
    const s = {
      step:        '10m',
      stepMinutes: 10,
      covered:     2,
      expected:    3,
      options:     [],
      defaultStep: '10m',
      points:      [
        {
          start:   '2026-09-24T00:00:00Z',
          minutes: 10,
          depts:   {
            rd: {
              cpu: 0.5, ram: null, cpuReq: 1, cpuUse: 0.5
            }
          }
        },
        { start: '2026-09-24T00:10:00Z', depts: null },
        {
          start:   '2026-09-24T00:20:00Z',
          minutes: 30,
          depts:   {
            rd: {
              cpu: 0.1, ram: null, cpuReq: 1, cpuUse: 0.1
            }
          }
        },
      ],
    } as unknown as Parameters<typeof averages>[0];
    const a = averages(s, ['rd', 'mfg'], 'cpu');

    expect(a).toHaveLength(1);
    expect(a[0].id).toBe('rd');
    expect(a[0].avg).toBeCloseTo(20);
    expect(averages(s, ['rd'], 'ram')).toStrictEqual([]);
  });
});
