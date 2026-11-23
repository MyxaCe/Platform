'use strict';
(function () {
  function create() {
    function authHeaders() {
      const t = window.Sso && window.Sso.token && window.Sso.token();
      return t ? { Authorization: 'Bearer ' + t } : {};
    }

    /**
     * @param {string} url
     * @returns {Promise<{ data: any, reason: string | null, status: number }>}
     */
    async function readDetailed(url) {
      let r;
      try {
        r = await fetch(url, { headers: authHeaders() });
      } catch {
        return { data: null, reason: 'network', status: 0 };
      }
      if (!r.ok) {
        return { data: null, reason: 'http-error', status: r.status };
      }
      try {
        return { data: await r.json(), reason: null, status: r.status };
      } catch {
        return { data: null, reason: 'malformed', status: r.status };
      }
    }

    /**
     * @param {string} url
     * @returns {Promise<any>}
     */
    async function read(url) {
      return (await readDetailed(url)).data;
    }

    /**
     * @param {string} url
     * @param {string} method
     * @param {any} [body]
     * @returns {Promise<{ ok: boolean, status: number, data: any }>}
     */
    async function mutate(url, method, body) {
      /** @type {Record<string, string>} */
      const headers = { ...authHeaders() };
      /** @type {RequestInit} */
      const opts = { method, headers };
      if (body !== undefined) {
        headers['Content-Type'] = 'application/json';
        opts.body = JSON.stringify(body);
      }
      try {
        const r = await fetch(url, opts);
        let data = null;
        try {
          data = await r.json();
        } catch {
        }
        return { ok: r.ok, status: r.status, data };
      } catch {
        return { ok: false, status: 0, data: null };
      }
    }

    function tradingBlocked() {
      const embedded = ((window.TERMINAL_CONFIG || {}).cabinetOrigins || []).some(Boolean);
      return embedded && !(window.Sso && window.Sso.token());
    }

    let status = 'unknown';

    /**
     * @param {string} url
     * @returns {Promise<any>}
     */
    async function authedRead(url) {
      if (tradingBlocked()) {
        status = 'no-session';
        return null;
      }
      try {
        const r = await fetch(url, { headers: authHeaders() });
        if (r.status === 401 || r.status === 403) {
          status = 'no-session';
          return null;
        }
        if (!r.ok) {
          status = 'unreachable';
          return null;
        }
        status = 'ok';
        return await r.json();
      } catch {
        status = 'unreachable';
        return null;
      }
    }

    /**
     * @param {string} url
     * @param {string} method
     * @param {any} [body]
     * @returns {Promise<{ ok: boolean, status: number, data: any }>}
     */
    async function authedMutate(url, method, body) {
      if (tradingBlocked()) {
        status = 'no-session';
        return { ok: false, status: 401, data: null };
      }
      const res = await mutate(url, method, body);
      if (res.status === 401 || res.status === 403) status = 'no-session';
      else if (res.status === 0) status = 'unreachable';
      return res;
    }

    return {
      tradingStatus: () => status,

      health: () => read('/health'),
      /** @param {string} [site] */
      instruments: (site) => read('/instruments' + (site ? `?site=${encodeURIComponent(site)}` : '')),
      /**
       * @param {number} id
       * @param {number} tf
       * @param {number} limit
       */
      /**
       * @param {number} id
       * @param {number} tf
       * @param {number} limit
       * @param {number} [end] unix-секунды: свечи не позже этого времени (подгрузка истории)
       */
      candles: (id, tf, limit, end) => read(`/candles/${id}?tf=${tf}&limit=${limit}${end != null ? `&end=${end}` : ''}`),
      /**
       * @param {number} id
       * @param {number} [limit]
       */
      depth: (id, limit) => read(`/depth/${id}${limit ? `?limit=${limit}` : ''}`),
      /** @param {number} id */
      stats: (id) => read(`/stats/${id}`),

      /**
       * @param {number} id
       * @param {number} [limit]
       */
      depthDetailed: (id, limit) => readDetailed(`/depth/${id}${limit ? `?limit=${limit}` : ''}`),
      /** @param {number} id */
      statsDetailed: (id) => readDetailed(`/stats/${id}`),

      account: () => authedRead('/account'),

      deals: () => authedRead('/deals'),
      /** @param {any} body */
      openDeal: (body) => authedMutate('/deals', 'POST', body),
      /** @param {number} id */
      closeDeal: (id) => authedMutate(`/deals/${id}/close`, 'POST'),
      closedDeals: () => authedRead('/deals/closed'),

      pending: () => authedRead('/pending'),
      /** @param {any} body */
      placePending: (body) => authedMutate('/pending', 'POST', body),
      /** @param {number} id */
      cancelPending: (id) => authedMutate(`/pending/${id}`, 'DELETE'),

      /**
       * @param {number} id
       * @param {{ sl?: number|null, tp?: number|null }} body
       */
      editDealLevels: (id, body) => authedMutate(`/deals/${id}/levels`, 'PUT', body),
      /**
       * @param {number} id
       * @param {{ sl?: number|null, tp?: number|null }} body
       */
      editPendingLevels: (id, body) => authedMutate(`/pending/${id}/levels`, 'PUT', body),
    };
  }

  window.Api = { create };
})();
