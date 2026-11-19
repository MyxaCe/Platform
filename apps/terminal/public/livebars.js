'use strict';

(function () {
  /**
   * @typedef {{ time: number, open: number, high: number, low: number, close: number, volume: number }} Bar
   */

  var CAP = 2000;

  /**
   * @param {Bar[]} rows
   * @param {Bar} bar
   * @param {number} [cap]
   * @returns {'same' | 'new' | 'old'}
   */
  function upsert(rows, bar, cap) {
    var max = cap || CAP;
    var row = { time: bar.time, open: bar.open, high: bar.high, low: bar.low, close: bar.close, volume: bar.volume };
    var n = rows.length;
    if (n && bar.time < rows[n - 1].time) return 'old';
    if (n && bar.time === rows[n - 1].time) {
      rows[n - 1] = row;
      return 'same';
    }
    rows.push(row);
    if (rows.length > max) rows.splice(0, rows.length - max);
    return 'new';
  }

  /**
   * @param {Bar[]} rows
   * @param {number} time
   * @returns {number}
   */
  function indexFrom(rows, time) {
    var i = rows.length;
    while (i > 0 && rows[i - 1].time >= time) i -= 1;
    return Math.min(i, Math.max(0, rows.length - 1));
  }

  /**
   * @param {(number | null | undefined)[]} values
   * @param {number[]} times
   * @param {number} from
   * @returns {{ time: number, value: number }[]}
   */
  function tail(values, times, from) {
    /** @type {{ time: number, value: number }[]} */
    var out = [];
    for (var i = Math.max(0, from); i < values.length; i += 1) {
      var v = values[i];
      if (v != null && isFinite(v)) out.push({ time: times[i], value: v });
    }
    return out;
  }

  window.LiveBars = { CAP: CAP, upsert: upsert, indexFrom: indexFrom, tail: tail };
})();
