'use strict';
(function () {
  const BPS = 10000n;

  function roundDiv(num, den) {
    return (num + den / 2n) / den;
  }

  /**
   * @param {boolean} buy
   * @param {number} market
   * @param {number} bps
   * @returns {number}
   */
  function execPrice(buy, market, bps) {
    if (!Number.isInteger(market) || !(bps > 0)) return market;
    const off = Number(roundDiv(BigInt(market) * BigInt(bps), BPS));
    return buy ? market + off : market - off;
  }

  /**
   * @param {number} exec
   * @param {number} qty
   * @param {number} pd
   * @param {number} qd
   * @param {number} bps
   * @param {number} [minorUnits=2]
   * @returns {number}
   */
  function commissionMinor(exec, qty, pd, qd, bps, minorUnits = 2) {
    if (!(bps > 0) || !Number.isInteger(exec) || !Number.isInteger(qty)) return 0;
    const notional = (BigInt(exec) * BigInt(qty) * 10n ** BigInt(minorUnits))
      / 10n ** BigInt(pd + qd);
    return Number(roundDiv(notional * BigInt(bps), BPS));
  }

  /**
   * @param {number} minor
   * @param {number} bps
   * @returns {string}
   */
  function commissionText(minor, bps) {
    if (!(bps > 0)) return '';
    if (!Number.isFinite(minor)) return 'Commission ≈ — USD';
    return `Commission ≈ ${(minor / 100).toFixed(2)} USD`;
  }

  window.Tariff = { execPrice, commissionMinor, commissionText };
})();
