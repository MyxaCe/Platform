'use strict';
/* ============================================================================
 * tariff.js — тариф счёта в тикете ордера (решение владельца 10.11).
 *
 * Доход площадки: наценка на спред (портфель Standard) и комиссия за сделку
 * (Pro / ENC Prime). Ставки — у счёта, в базисных пунктах, приезжают в
 * `/account` (`markup_bps`, `commission_bps`). Исполняет и списывает шлюз;
 * здесь — только ПОКАЗ того, что шлюз сделает, той же арифметикой:
 *
 *   цена исполнения  покупка = ask + round(ask · bps / 10000),
 *                    продажа = bid − round(bid · bps / 10000);
 *   комиссия         round(цена исполнения · объём · 10^minor / 10^(pd+qd)
 *                          · bps / 10000), округление к ближайшему.
 *
 * Отдельной строки «наценка» в тикете нет — так принято на spread-счетах:
 * человек видит цену, по которой исполнится сделка. Комиссия — строкой
 * «Commission ≈ X USD» до подтверждения.
 *
 * Отдельным файлом, а не внутри app.js: app.js трогает DOM с первой строки и в
 * Node не поднимается, а арифметику денег, которую нельзя проверить, мы не
 * держим (тот же довод, что у rejection.js).
 * ========================================================================== */
(function () {
  const BPS = 10000n;

  /** Округление к ближайшему (половина — от нуля) для неотрицательных. */
  function roundDiv(num, den) {
    return (num + den / 2n) / den;
  }

  /**
   * Цена исполнения с наценкой, в целых единицах точности инструмента.
   * @param {boolean} buy сторона ИСПОЛНЕНИЯ: покупка — дороже рынка
   * @param {number} market рыночная цена (ask для покупки, bid для продажи)
   * @param {number} bps наценка, базисные пункты
   * @returns {number}
   */
  function execPrice(buy, market, bps) {
    if (!Number.isInteger(market) || !(bps > 0)) return market;
    const off = Number(roundDiv(BigInt(market) * BigInt(bps), BPS));
    return buy ? market + off : market - off;
  }

  /**
   * Комиссия за исполнение, в наименьших единицах валюты счёта (центах).
   * @param {number} exec цена исполнения, целые единицы точности
   * @param {number} qty объём, целые единицы точности
   * @param {number} pd знаков цены
   * @param {number} qd знаков объёма
   * @param {number} bps комиссия, базисные пункты
   * @param {number} [minorUnits=2] знаков валюты счёта
   * @returns {number}
   */
  function commissionMinor(exec, qty, pd, qd, bps, minorUnits = 2) {
    if (!(bps > 0) || !Number.isInteger(exec) || !Number.isInteger(qty)) return 0;
    const notional = (BigInt(exec) * BigInt(qty) * 10n ** BigInt(minorUnits))
      / 10n ** BigInt(pd + qd);
    return Number(roundDiv(notional * BigInt(bps), BPS));
  }

  /**
   * Строка тикета. Пустая — комиссии нет (тариф без комиссии), и тогда строки
   * в тикете не должно быть вовсе, а не «≈ 0».
   * @param {number} minor комиссия в центах
   * @param {number} bps ставка
   * @returns {string}
   */
  function commissionText(minor, bps) {
    if (!(bps > 0)) return '';
    if (!Number.isFinite(minor)) return 'Commission ≈ — USD';
    return `Commission ≈ ${(minor / 100).toFixed(2)} USD`;
  }

  window.Tariff = { execPrice, commissionMinor, commissionText };
})();
