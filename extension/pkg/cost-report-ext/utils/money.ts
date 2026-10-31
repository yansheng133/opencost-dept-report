// One precision for every amount on a page. Deciding decimals per column
// showed the same department as 0.4714 in a bar and 0.47 in the table, and
// changed with the department filter; finance readers compare those
// numbers across the page. The page sets the precision once, from the
// report's total (or the Periods page's largest month), and every
// component formats with it.
import { ref } from 'vue';

/** Decimals for a page whose amounts reach `total`: ≥1000 → 0, ≥10 → 2, else 4. */
export function pageDigits(total: unknown): number {
  const n = Math.abs(Number(total ?? 0));

  return !isFinite(n) ? 2 : n >= 1000 ? 0 : n >= 10 ? 2 : 4;
}

/**
 * An amount at `digits` decimals with thousands separators. A non-zero
 * amount that rounds to zero is "<0.01" (at that precision), not "0.00":
 * a workload that costs a little did not cost nothing.
 */
export function moneyFixed(v: unknown, digits: number): string {
  const n = Number(v ?? 0);

  if (!isFinite(n)) {
    return '—';
  }
  const step = 10 ** -digits;

  if (n !== 0 && Math.abs(n) < step / 2) {
    const s = step.toFixed(digits);

    return n > 0 ? `<${ s }` : `>-${ s }`;
  }

  return n.toLocaleString('en-US', { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

/** The page's precision; set by the page when its data arrives. */
export const moneyDigits = ref(2);

export function setMoneyBasis(total: unknown) {
  moneyDigits.value = pageDigits(total);
}

/** An amount at the page's precision (reactive in templates). */
export function fmtMoney(v: unknown): string {
  return moneyFixed(v, moneyDigits.value);
}
