'use strict';

(function () {
  /** @type {Readonly<Record<string, string>>} */
  var BREAK_REASONS = Object.freeze({
    'feed-lost': 'связь с фидом оборвалась',
    'feed-stale': 'фид молчит дольше порога',
    'source-change': 'сменился источник котировок',
  });

  /** @typedef {{ time: number, open: number, high: number, low: number, close: number, volume: number }} Bar */

  /** @typedef {{ reason: string, closed: Bar | null }} BreakResult */

  function create() {
    /** @type {Bar | null} */
    var forming = null;
    var lastBarTime = 0;
    var broken = false;
    /** @type {string | null} */
    var breakReason = null;

    /**
     * @param {Bar} b
     * @returns {Bar}
     */
    function copy(b) {
      return { time: b.time, open: b.open, high: b.high, low: b.low, close: b.close, volume: b.volume };
    }

    return {
      /** @returns {Bar | null} */
      forming: function () {
        return forming ? copy(forming) : null;
      },
      /** @returns {number} */
      lastBarTime: function () {
        return lastBarTime;
      },
      /** @returns {boolean} */
      isBroken: function () {
        return broken;
      },
      /** @returns {string | null} */
      breakReason: function () {
        return breakReason;
      },

      /** @param {Bar | null} bar */
      seed: function (bar) {
        broken = false;
        breakReason = null;
        if (!bar) {
          forming = null;
          lastBarTime = 0;
          return;
        }
        forming = copy(bar);
        lastBarTime = bar.time;
      },

      /**
       * @param {{ now: number, tf: number, price: number, qty: number }} t
       * @returns {Bar | null}
       */
      onTrade: function (t) {
        if (broken) return null;
        var b = t.now - (t.now % t.tf);
        if (!forming || b > forming.time) {
          forming = { time: b, open: t.price, high: t.price, low: t.price, close: t.price, volume: t.qty };
        } else if (b < forming.time) {
          return null;
        } else {
          forming.high = Math.max(forming.high, t.price);
          forming.low = Math.min(forming.low, t.price);
          forming.close = t.price;
          forming.volume += t.qty;
        }
        if (forming.time < lastBarTime) return null;
        lastBarTime = forming.time;
        return copy(forming);
      },

      /**
       * @param {string} reason
       * @returns {BreakResult}
       */
      markBreak: function (reason) {
        if (!Object.prototype.hasOwnProperty.call(BREAK_REASONS, reason)) {
          throw new Error('неизвестная причина разрыва ряда: ' + reason);
        }
        var closed = forming ? copy(forming) : null;
        if (!broken) {
          broken = true;
          breakReason = reason;
        }
        return { reason: reason, closed: closed };
      },
    };
  }

  window.Series = { create: create, BREAK_REASONS: BREAK_REASONS };
})();
