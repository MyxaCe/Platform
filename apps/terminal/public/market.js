'use strict';
(function () {
  /** @typedef {'open'|'closed'|'halted'} Status */
  /** @typedef {{ status: Status, next_open: number|null, reason: string|null, tradable: boolean }} State */
  /** @typedef {{ open?: string, close?: string }} Slot */
  /** @typedef {{ timezone?: string, weekly?: Slot[], holidays?: string[] }} Session */
  /** @typedef {{ instrument: number, bid: number|null, ask: number|null, price: number, time: number|null }} Quote */

  /** @type {Status[]} */
  const STATUSES = ['open', 'closed', 'halted'];
  const DAYS = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'];
  const HORIZON_DAYS = 21;

  /** @param {any} v @returns {v is number} */
  const isMs = (v) => typeof v === 'number' && Number.isFinite(v) && v > 0;
  /** @param {any} v @returns {v is number} */
  const isRaw = (v) => Number.isInteger(v) && v > 0;

  /** @param {any} it @returns {State} */
  function state(it) {
    const m = it && it.market;
    if (!m || typeof m !== 'object') return { status: 'open', next_open: null, reason: null, tradable: true };
    /** @type {Status} */
    const status = STATUSES.includes(m.status) ? m.status : 'halted';
    return {
      status,
      next_open: isMs(m.next_open) ? m.next_open : null,
      reason: typeof m.reason === 'string' && m.reason ? m.reason : null,
      tradable: typeof m.tradable === 'boolean' ? m.tradable && status === 'open' : status === 'open',
    };
  }

  /** @param {any} it @returns {boolean} */
  function closeOnly(it) {
    return !!it && it.mode === 'closeOnly';
  }

  /** @param {any} it @returns {'closed'|'halted'|'untradable'|'closeOnly'|null} */
  function blockOpen(it) {
    if (!it) return null;
    const s = state(it);
    if (s.status === 'halted') return 'halted';
    if (s.status === 'closed') return 'closed';
    if (!s.tradable) return 'untradable';
    if (closeOnly(it)) return 'closeOnly';
    return null;
  }

  /** @param {any} it @returns {'closeOnly'|null} */
  function blockPending(it) {
    return closeOnly(it) ? 'closeOnly' : null;
  }

  /** @param {any} it @returns {'closed'|'halted'|null} */
  function blockClose(it) {
    if (!it) return null;
    const s = state(it).status;
    return s === 'closed' || s === 'halted' ? s : null;
  }

  /** @param {string} tz @returns {Intl.DateTimeFormat} */
  function partsFormat(tz) {
    return new Intl.DateTimeFormat('en-US', {
      timeZone: tz, hourCycle: 'h23', year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit',
    });
  }

  /** @param {number} ts @param {Intl.DateTimeFormat} f */
  function zoned(ts, f) {
    /** @type {Record<string, number>} */
    const p = {};
    for (const x of f.formatToParts(new Date(ts))) if (x.type !== 'literal') p[x.type] = Number(x.value);
    return { y: p.year, m: p.month, d: p.day, h: p.hour % 24, mi: p.minute, s: p.second };
  }

  /** @param {number} ts @param {Intl.DateTimeFormat} f */
  function offset(ts, f) {
    const z = zoned(ts, f);
    return Date.UTC(z.y, z.m - 1, z.d, z.h, z.mi, z.s) - Math.floor(ts / 1000) * 1000;
  }

  /** @param {number} y @param {number} m @param {number} d @param {number} h @param {number} mi @param {Intl.DateTimeFormat} f */
  function localToUtc(y, m, d, h, mi, f) {
    const guess = Date.UTC(y, m - 1, d, h, mi);
    const o1 = offset(guess, f);
    const at = guess - o1;
    const o2 = offset(at, f);
    return o2 === o1 ? at : guess - o2;
  }

  /** @param {any} s @returns {{ day: number, h: number, mi: number } | null} */
  function parseSlot(s) {
    const m = /^\s*([a-z]{3})\s+(\d{1,2}):(\d{2})\s*$/i.exec(typeof s === 'string' ? s : '');
    if (!m) return null;
    const day = DAYS.indexOf(m[1].toLowerCase());
    const h = Number(m[2]), mi = Number(m[3]);
    if (day < 0 || h > 24 || mi > 59) return null;
    return { day, h, mi };
  }

  /** @param {Session|null|undefined} session @param {number} now @returns {number|null} */
  function fromSession(session, now) {
    if (!session || typeof session !== 'object' || !Array.isArray(session.weekly) || !session.weekly.length) return null;
    /** @type {{ day: number, h: number, mi: number }[]} */
    const opens = [];
    for (const w of session.weekly) { const o = parseSlot(w && w.open); if (o) opens.push(o); }
    if (!opens.length) return null;
    let f;
    try { f = partsFormat(session.timezone || 'UTC'); } catch { return null; }
    const holidays = Array.isArray(session.holidays) ? session.holidays : [];
    const today = zoned(now, f);
    for (let k = 0; k <= HORIZON_DAYS; k += 1) {
      const day = new Date(Date.UTC(today.y, today.m - 1, today.d + k));
      if (holidays.includes(day.toISOString().slice(0, 10))) continue;
      /** @type {number|null} */
      let best = null;
      for (const o of opens) {
        if (o.day !== day.getUTCDay()) continue;
        const at = localToUtc(day.getUTCFullYear(), day.getUTCMonth() + 1, day.getUTCDate(), o.h, o.mi, f);
        if (at > now && (best === null || at < best)) best = at;
      }
      if (best !== null) return best;
    }
    return null;
  }

  /** @param {any} it @param {number} now @returns {number|null} */
  function nextOpen(it, now) {
    const s = state(it);
    if (s.status !== 'closed') return null;
    if (s.next_open !== null) return s.next_open;
    return fromSession(it && it.session, now);
  }

  /** @param {number} ms @param {string} locale @param {number} now @param {string} [timeZone] @returns {string} */
  function when(ms, locale, now, timeZone) {
    const day = (/** @type {number} */ t) => new Date(t).toLocaleDateString('en-CA', { timeZone });
    const time = { hour: /** @type {'2-digit'} */ ('2-digit'), minute: /** @type {'2-digit'} */ ('2-digit'), hourCycle: /** @type {'h23'} */ ('h23'), timeZone };
    if (day(ms) === day(now)) return new Date(ms).toLocaleTimeString(locale, time);
    return new Date(ms).toLocaleString(locale, Object.assign({ weekday: /** @type {'short'} */ ('short'), day: /** @type {'numeric'} */ ('numeric'), month: /** @type {'short'} */ ('short') }, time));
  }

  /** @param {any} x @returns {Quote|null} */
  function quoteOf(x) {
    if (!x || typeof x !== 'object' || x.type !== 'quote' || !Number.isInteger(x.instrument)) return null;
    const bid = isRaw(x.bid) ? x.bid : null;
    const ask = isRaw(x.ask) ? x.ask : null;
    const price = isRaw(x.price) ? x.price : bid !== null && ask !== null ? Math.round((bid + ask) / 2) : null;
    if (price === null) return null;
    return { instrument: x.instrument, bid, ask, price, time: isMs(x.time) ? x.time : null };
  }

  /** @param {any} x @returns {{ instrument: number, market: State } | null} */
  function statusOf(x) {
    if (!x || typeof x !== 'object' || x.type !== 'market' || !Number.isInteger(x.instrument)) return null;
    if (!STATUSES.includes(x.status)) return null;
    return { instrument: x.instrument, market: state({ market: x }) };
  }

  /** @param {any} v @returns {string} */
  function normSymbol(v) {
    return typeof v === 'string' ? v.toUpperCase().replace(/[^A-Z0-9]/g, '') : '';
  }

  /** @param {any[]|null|undefined} list @param {any} want @returns {number|null} */
  function matchSymbol(list, want) {
    const n = normSymbol(want);
    if (!n || !Array.isArray(list)) return null;
    for (const it of list) {
      if (it && (normSymbol(it.canonical) === n || normSymbol(it.symbol) === n)) return it.id;
    }
    for (const it of list) {
      const s = it ? normSymbol(it.symbol) : '';
      if (s.endsWith('USDT') && s.slice(0, -1) === n) return it.id;
    }
    return null;
  }

  /** @param {any} it @returns {[string, string]} */
  function pair(it) {
    if (!it || typeof it.symbol !== 'string') return ['', ''];
    const s = it.symbol;
    if (s.includes('-')) { const [b, q] = s.split('-'); return [b, q || '']; }
    if (typeof it.base === 'string' && it.base) return [it.base, typeof it.quote === 'string' ? it.quote : ''];
    if (it.category === 'stocks' || it.category === 'indices') return [s, 'USD'];
    if (/^[A-Z]{6}$/.test(s)) return [s.slice(0, 3), s.slice(3)];
    return [s, ''];
  }

  window.Market = { pair, state, closeOnly, blockOpen, blockPending, blockClose, fromSession, nextOpen, when, quoteOf, statusOf, normSymbol, matchSymbol, STATUSES };
})();
