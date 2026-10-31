// Geometry of the usage trend chart (使用率趨勢), kept apart from the SVG so
// it can be tested. Usage = actual use ÷ requests, per department and
// segment of `step`.
//
// Rules carried over from the Python page, each found by getting it wrong:
// - X is real time, not the point's index: segments without data would
//   otherwise spread the rest evenly, and a 6-hour gap would look the same
//   as a 3-day one.
// - A segment without data breaks the line. Joining the two sides would
//   look measured, and the gap may be monitoring being down.
// - The last segment while it has not ended yet is drawn dashed: its
//   minutes are short and the last few minutes are not counted yet.
import type { Series, SeriesPoint } from './types';

export type Metric = 'cpu' | 'ram';

export const BOX = {
  W: 860, H: 260, L: 46, R: 74, T: 14, B: 26
};

export interface Pt extends SeriesPoint { ts: number }

export type XY = [number, number];

export interface Line {
  id: string;
  segments: XY[][];
  // The unfinished last segment, joined from the previous point if any.
  tail: { from: XY | null; to: XY } | null;
  // Small dots per sample, unless they would be too dense to read.
  marks: boolean;
}

export interface Chart {
  pts: Pt[];
  t0: number;
  t1: number;
  maxY: number;
  partialIdx: number;
  lines: Line[];
  yTicks: { v: number; y: number }[];
  xTicks: { x: number; start: string; withDate: boolean }[];
  endLabels: { id: string; y: number }[];
}

export function xOf(ts: number, t0: number, t1: number): number {
  const span = Math.max(1, t1 - t0);

  return BOX.L + (BOX.W - BOX.L - BOX.R) * (ts - t0) / span;
}

export function yOf(v: number, maxY: number): number {
  return BOX.T + (BOX.H - BOX.T - BOX.B) * (1 - v / maxY);
}

/** Points with a usable time, in order. */
export function timedPoints(s: Series | null | undefined): Pt[] {
  return (s?.points || [])
    .filter((p) => p.start)
    .map((p) => ({ ...p, ts: Date.parse(p.start) }))
    .filter((p) => !isNaN(p.ts));
}

/** Usage of one department in one point, in percent; null: nothing to draw. */
export function valueOf(p: SeriesPoint, id: string, metric: Metric): number | null {
  if (!p.depts) {
    return null;
  }
  const v = p.depts[id]?.[metric];

  return v === null || v === undefined ? null : v * 100;
}

/**
 * Index of the last point if its segment has not ended yet (fewer minutes
 * than the step and its end still in the future), else -1. A past day's
 * last segment with few minutes is missing data, not unfinished.
 */
export function partialIndex(pts: Pt[], stepMinutes: number, now: number): number {
  const last = pts[pts.length - 1];

  if (!last || !last.depts) {
    return -1;
  }

  return (last.minutes ?? 0) < stepMinutes * 0.9 && last.ts + stepMinutes * 60000 > now ? pts.length - 1 : -1;
}

/** Spread end labels that would overlap, inside the plot area. */
export function spreadLabels(labels: { id: string; y: number }[], minGap = 14): { id: string; y: number }[] {
  const top = BOX.T + 4;
  const bottom = BOX.H - BOX.B;
  const arr = labels.map((l) => ({ ...l })).sort((a, b) => a.y - b.y);

  arr.forEach((l, i) => {
    if (i && l.y - arr[i - 1].y < minGap) {
      l.y = arr[i - 1].y + minGap;
    }
  });
  // Pushed past the bottom: move back up from the last one, only as far as
  // needed (clamping each to the edge would stack them on one spot again).
  if (arr.length && arr[arr.length - 1].y > bottom) {
    arr[arr.length - 1].y = bottom;
    for (let i = arr.length - 2; i >= 0; i--) {
      if (arr[i + 1].y - arr[i].y < minGap) {
        arr[i].y = arr[i + 1].y - minGap;
      }
    }
  }

  return arr.map((l) => ({ ...l, y: Math.max(l.y, top) }));
}

export function buildChart(s: Series, depts: string[], metric: Metric, now = Date.now()): Chart | null {
  const pts = timedPoints(s);

  if (!pts.length) {
    return null;
  }
  const t0 = pts[0].ts;
  const t1 = pts[pts.length - 1].ts;
  const partialIdx = partialIndex(pts, s.stepMinutes, now);
  let max = 0;

  pts.forEach((p) => depts.forEach((id) => {
    const v = valueOf(p, id, metric);

    if (v !== null) {
      max = Math.max(max, v);
    }
  }));
  // At least up to 20%, so noise does not look like mountains.
  const maxY = Math.max(20, Math.ceil(max / 20) * 20);
  const lines: Line[] = depts.map((id) => {
    const segments: XY[][] = [];
    let cur: XY[] = [];
    let tail: Line['tail'] = null;

    pts.forEach((p, i) => {
      const v = valueOf(p, id, metric);

      if (v === null) {
        if (cur.length) {
          segments.push(cur);
        }
        cur = [];

        return;
      }
      const xy: XY = [xOf(p.ts, t0, t1), yOf(v, maxY)];

      if (i === partialIdx) {
        tail = { from: cur.length ? cur[cur.length - 1] : null, to: xy };

        return;
      }
      cur.push(xy);
    });
    if (cur.length) {
      segments.push(cur);
    }
    // Dots closer than 3 units only blur into a thick line.
    const marks = segments.some((seg) => seg.length > 1 && (seg[seg.length - 1][0] - seg[0][0]) / (seg.length - 1) >= 3);

    return {
      id, segments, tail, marks
    };
  });
  const yTicks = [0, 1, 2, 3, 4].map((i) => ({ v: maxY * i / 4, y: yOf(maxY * i / 4, maxY) }));
  // At most five time labels; the date is added when the whole span is
  // over a day (not when one step is: 7 days at 1 h would show only times).
  const every = Math.max(1, Math.ceil(pts.length / 5));
  const xTicks = pts
    .map((p, i) => ({ p, i }))
    .filter(({ i }) => !(i % every) || i === pts.length - 1)
    .map(({ p }) => ({
      x: xOf(p.ts, t0, t1), start: p.start, withDate: t1 - t0 > 864e5
    }));
  const ends = lines
    .map((l) => {
      const seg = l.segments[l.segments.length - 1];

      return seg ? { id: l.id, y: seg[seg.length - 1][1] } : null;
    })
    .filter((l): l is { id: string; y: number } => !!l);

  return {
    pts, t0, t1, maxY, partialIdx, lines, yTicks, xTicks, endLabels: spreadLabels(ends)
  };
}

/** The point nearest to an x position (SVG units), for the hover tip. */
export function nearestPoint(c: Chart, x: number): number {
  let best = 0;
  let bestD = Infinity;

  c.pts.forEach((p, i) => {
    const d = Math.abs(xOf(p.ts, c.t0, c.t1) - x);

    if (d < bestD) {
      bestD = d;
      best = i;
    }
  });

  return best;
}

export const pathOf = (seg: XY[]) => seg.map(([x, y], i) => `${ i ? 'L' : 'M' }${ x.toFixed(1) } ${ y.toFixed(1) }`).join(' ');

/**
 * The step to ask for: the user's choice when this period offers it and it
 * is not what the report already has; else null (use the report's). A
 * choice not offered here (5m chosen on 24 h, now on 7 d) is kept for
 * when the user goes back.
 */
export function wantedStep(chosen: string | null, s: Series | null | undefined): string | null {
  return chosen && s && (s.options || []).includes(chosen) && chosen !== s.step ? chosen : null;
}
