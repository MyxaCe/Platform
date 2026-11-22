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

  /**
   * @param {any} account
   * @param {any} it
   * @returns {number|null}
   */
  function leverageFor(account, it) {
    if (!account || !Number.isInteger(account.leverage) || account.leverage < 1) return null;
    const table = account.leverage_classes;
    const cat = it && typeof it.category === 'string' ? it.category : null;
    const lev = table && cat && Object.prototype.hasOwnProperty.call(table, cat) ? table[cat] : account.leverage;
    return Number.isInteger(lev) && lev >= 1 ? lev : account.leverage;
  }

  window.Tariff = { execPrice, commissionMinor, commissionText, leverageFor };
})();
