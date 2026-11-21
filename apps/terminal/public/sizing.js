'use strict';
(function () {
  const DEFAULT_QTY_DECIMALS = 3;
  const MAX_DECIMALS = 8;

  /** @param {any} it @returns {number} */
  function qtyDecimals(it) {
    return it && Number.isInteger(it.qty_decimals) && it.qty_decimals >= 0 ? it.qty_decimals : DEFAULT_QTY_DECIMALS;
  }

  /** @param {any} it @returns {number|null} */
  function contract(it) {
    if (!it || it.contract_size == null) return null;
    const cs = Number(it.contract_size);
    return Number.isFinite(cs) && cs > 1 ? cs : null;
  }

  /** @param {any} it @returns {number} */
  function step(it) {
    return it && Number.isInteger(it.qty_step) && it.qty_step > 0 ? it.qty_step : 1;
  }

  /** @param {any} it @returns {number|null} */
  function minQty(it) {
    return it && Number.isInteger(it.min_qty) && it.min_qty > 0 ? it.min_qty : null;
  }

  /** @param {number} v @returns {number} */
  function decimalsOf(v) {
    for (let d = 0; d <= MAX_DECIMALS; d += 1) {
      if (Math.abs(Math.round(v * 10 ** d) - v * 10 ** d) < 1e-6) return d;
    }
    return MAX_DECIMALS;
  }

  /** @param {any} it @param {number} raw @returns {number} */
  function toUnits(it, raw) {
    return raw / 10 ** qtyDecimals(it);
  }

  /** @param {any} it @param {number} raw @returns {number} */
  function toLots(it, raw) {
    return toUnits(it, raw) / (contract(it) || 1);
  }

  /** @param {any} it @returns {number} */
  function lotDecimals(it) {
    if (it && Number.isInteger(it.lot_decimals) && it.lot_decimals >= 0) return it.lot_decimals;
    return decimalsOf(toLots(it, step(it)));
  }

  /** @param {any} it @param {number} units @returns {number} */
  function fromUnits(it, units) {
    if (!(units > 0) || !Number.isFinite(units)) return 0;
    const raw = Math.round(units * 10 ** qtyDecimals(it));
    const s = step(it);
    return s > 1 ? Math.floor(raw / s) * s : raw;
  }

  /** @param {any} it @param {number} lots @returns {number} */
  function fromLots(it, lots) {
    return fromUnits(it, lots * (contract(it) || 1));
  }

  /** @param {any} it @param {number} raw @returns {boolean} */
  function belowMin(it, raw) {
    const m = minQty(it);
    return m !== null && raw < m;
  }

  /** @param {any} it @param {number} raw @param {(v: number, d: number) => string} num @returns {string} */
  function format(it, raw, num) {
    return contract(it) ? num(toLots(it, raw), lotDecimals(it)) : num(toUnits(it, raw), qtyDecimals(it));
  }

  window.Sizing = { contract, step, minQty, qtyDecimals, lotDecimals, toUnits, toLots, fromUnits, fromLots, belowMin, format };
})();
