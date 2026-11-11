'use strict';
/* ============================================================================
 * stops.js — минимальное расстояние SL/TP от цены в тикете ордера.
 *
 * Минимум задаёт площадка (админка сайта → шлюз `settings/stops`), тикет
 * получает его в `/account` (`min_stop_bps`). Граница — на шлюзе: ближе
 * минимума он отказывает `422 stop_loss_too_close | take_profit_too_close`.
 * Здесь — та же арифметика, чтобы тикет подставлял уровни, которые шлюз
 * примет, и не давал отправить те, которые не примет:
 *
 *   расстояние  d = ceil(цена · bps / 10000)        (вверх, как на шлюзе)
 *   покупка     SL ≤ цена − d,  TP ≥ цена + d
 *   продажа     SL ≥ цена + d,  TP ≤ цена − d
 *
 * Цена отсчёта — та же, что у шлюза: рыночная цена стороны (ask для покупки,
 * bid для продажи), у лимитного ордера — цена ордера.
 *
 * Подстановка. Уровни ставятся на минимальном расстоянии. У рыночного ордера
 * к нему добавляется запас 10 % от минимума: между экраном и шлюзом цена
 * успевает сдвинуться, и уровень ровно на границе отказал бы на первом же
 * тике. Вручную можно поставить и точно на границу.
 *
 * Минимум 0 (ограничения нет) — подставляется подсказка 1 % (`HINT_BPS`):
 * значение, которое заметно на графике любого инструмента и не выбивает
 * позицию шумом спреда.
 *
 * Отдельным файлом по той же причине, что tariff.js: app.js в Node не
 * поднимается, а арифметику, которую нельзя проверить, мы не держим.
 * ========================================================================== */
(function () {
  /** Подсказка, когда площадка минимума не задала: 1 %. */
  const HINT_BPS = 100;
  /** Запас подстановки у рыночного ордера: доля минимума. */
  const MARKET_BUFFER = 0.1;

  /**
   * Минимальное расстояние в целых единицах цены.
   * @param {number} ref цена отсчёта, целые единицы точности
   * @param {number} bps
   * @returns {number}
   */
  function minDistance(ref, bps) {
    if (!(bps > 0) || !Number.isInteger(ref) || ref <= 0) return 0;
    const num = BigInt(ref) * BigInt(bps);
    return Number((num + 9999n) / 10000n);
  }

  /**
   * Проверка уровней. `null` у уровня — уровня нет.
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
   * Уровни для подстановки.
   * @param {'buy'|'sell'} side
   * @param {number} ref
   * @param {number} bps минимум площадки (0 — нет)
   * @param {boolean} market рыночный ордер: с запасом на движение цены
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
