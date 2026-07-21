'use strict';
// ============================================================================
// api.js — клиент REST-API терминала. Модуль-библиотека по ADR-015:
// ноль DOM, ноль чужих глобалов; всё внешнее приходит через ctx.
//
// Зачем отдельный модуль: заголовок авторизации и разбор ответа были продублированы
// в восьми местах app.js, а токен читался прямо из элемента страницы. Настоящий auth
// (P0 в backlog) поменяет заголовки, добавит обновление сессии и обработку 401 —
// с этим модулем правка будет в одном месте, а не в каждом виджете.
//
// Контракт:
//   const api = Api.create({ token: () => 'bearer-token' });
//
// Соглашение по ошибкам (повторяет прежнее поведение app.js):
//   • чтение   → данные при успехе, null при сетевой ошибке или не-2xx.
//     null означает «данных нет», и вызывающий оставляет на экране прежние —
//     это важнее пустого списка: при обрыве связи UI не должен показывать
//     «нет позиций», когда позиции есть.
//   • мутация  → { ok, status, data } — вызывающий сам решает, что показать.
// ============================================================================
(function () {
  /** Создать клиента. `ctx.token()` вызывается на каждом запросе — токен может смениться. */
  function create(ctx) {
    const token = (ctx && ctx.token) || (() => '');
    const auth = () => ({ Authorization: `Bearer ${token()}` });

    /** GET с разбором JSON. Возвращает данные либо null (ошибка сети / не-2xx). */
    async function read(url, headers) {
      try {
        const r = await fetch(url, headers ? { headers } : undefined);
        if (!r.ok) return null;
        return await r.json();
      } catch {
        return null;
      }
    }

    /** POST/DELETE с телом. Возвращает { ok, status, data }; data может быть null. */
    async function mutate(url, method, body) {
      const opts = { method, headers: auth() };
      if (body !== undefined) {
        opts.headers['Content-Type'] = 'application/json';
        opts.body = JSON.stringify(body);
      }
      try {
        const r = await fetch(url, opts);
        let data = null;
        try {
          data = await r.json();
        } catch {
          /* пустое тело (204 No Content) — норма */
        }
        return { ok: r.ok, status: r.status, data };
      } catch {
        return { ok: false, status: 0, data: null }; // сеть недоступна
      }
    }

    return {
      // ---- Публичные данные (без авторизации) ----------------------------
      instruments: () => read('/instruments'),
      candles: (id, tf, limit) => read(`/candles/${id}?tf=${tf}&limit=${limit}`),
      depth: (id, limit) => read(`/depth/${id}${limit ? `?limit=${limit}` : ''}`),
      stats: (id) => read(`/stats/${id}`),

      // ---- Счёт пользователя ---------------------------------------------
      account: () => read('/account', auth()),

      // ---- Позиции ---------------------------------------------------------
      deals: () => read('/deals', auth()),
      openDeal: (body) => mutate('/deals', 'POST', body),
      closeDeal: (id) => mutate(`/deals/${id}/close`, 'POST'),
      closedDeals: () => read('/deals/closed', auth()),

      // ---- Отложенные (лимитные) ордера -----------------------------------
      pending: () => read('/pending', auth()),
      placePending: (body) => mutate('/pending', 'POST', body),
      cancelPending: (id) => mutate(`/pending/${id}`, 'DELETE'),
    };
  }

  window.Api = { create };
})();
