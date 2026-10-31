import { describe, it, expect } from '@jest/globals';
import { pageDigits, moneyFixed, fmtMoney, setMoneyBasis } from '../money';

describe('page money', () => {
  it('decides the decimals once from the total', () => {
    expect(pageDigits(25.03)).toBe(2);
    expect(pageDigits(1234)).toBe(0);
    expect(pageDigits(4.9)).toBe(4);
    expect(pageDigits(null)).toBe(4);
  });

  it('formats every amount the same way, small ones as "<"', () => {
    expect(moneyFixed(0.4714, 2)).toBe('0.47');
    expect(moneyFixed(1234.5, 0)).toBe('1,235');
    expect(moneyFixed(0.004, 2)).toBe('<0.01');
    expect(moneyFixed(-0.004, 2)).toBe('>-0.01');
    expect(moneyFixed(0, 2)).toBe('0.00');
    expect(moneyFixed(0.3, 0)).toBe('<1');
    expect(moneyFixed(NaN, 2)).toBe('—');
    setMoneyBasis(25);
    expect(fmtMoney(0.8284)).toBe('0.83');
    setMoneyBasis(3);
    expect(fmtMoney(0.8284)).toBe('0.8284');
  });
});
