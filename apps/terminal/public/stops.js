'use strict';
(function () {
  const HINT_BPS = 100;
  const MARKET_BUFFER = 0.1;

  /**
   * @param {number} ref
   * @param {number} bps
   * @returns {number}
   */
  function minDistance(ref, bps) {
    if (!(bps > 0) || !Number.isInteger(ref) || ref <= 0) return 0;
    const num = BigInt(ref) * BigInt(bps);
    return Number((num + 9999n) / 10000n);
  }

  /**
   * @param {'buy'|'sell'} side
   * @param {number} ref
   * @param {number|null} sl
   * @param {number|null} tp
   * @param {number} bps
   * @returns {null | { code: 'stop_loss_too_close'|'take_profit_too_close', limit: number, distance: number }}
   */
  function check(side, ref, sl, tp, bps) {
    const d = minDistance(ref, bps);
    if (d === 0) return null;
    const buy = side === 'buy';
    const slLimit = buy ? ref - d : ref + d;
    const tpLimit = buy ? ref + d : ref - d;
    if (sl != null && !(buy ? sl <= slLimit : sl >= slLimit)) return { code: 'stop_loss_too_close', limit: slLimit, distance: d };
    if (tp != null && !(buy ? tp >= tpLimit : tp <= tpLimit)) return { code: 'take_profit_too_close', limit: tpLimit, distance: d };
    return null;
  }

  /**
   * @param {'buy'|'sell'} side
   * @param {number} ref
   * @param {number} bps
   * @param {boolean} market
   * @returns {{ sl: number, tp: number, distance: number, hint: boolean } | null}
   */
  function suggest(side, ref, bps, market) {
    if (!Number.isInteger(ref) || ref <= 0) return null;
    const hint = !(bps > 0);
    const base = minDistance(ref, hint ? HINT_BPS : bps);
    const d = !hint && market ? base + Math.max(1, Math.ceil(base * MARKET_BUFFER)) : base;
    const buy = side === 'buy';
    const sl = buy ? ref - d : ref + d;
    const tp = buy ? ref + d : ref - d;
    if (sl <= 0 || tp <= 0) return null;
    return { sl, tp, distance: minDistance(ref, hint ? 0 : bps), hint };
  }

  window.Stops = { minDistance, check, suggest, HINT_BPS };
})();
