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
//   const api = Api.create();  // терминал автономный, авторизации нет
//
// Соглашение по ошибкам (повторяет прежнее поведение app.js):
//   • чтение   → данные при успехе, null при сетевой ошибке или не-2xx.
//     null означает «данных нет», и вызывающий оставляет на экране прежние —
//     это важнее пустого списка: при обрыве связи UI не должен показывать
//     «нет позиций», когда позиции есть.
//   • мутация  → { ok, status, data } — вызывающий сам решает, что показать.
// ============================================================================
(function () {
  /** Создать клиента. Терминал автономный (пивот 2026-07-25): логина нет, все запросы
   *  идут на тот же origin к одному демо-счёту по умолчанию. */
  function create() {
    /** Заголовок сессии терминала (ADR-023, Т2): bearer в памяти, если вошли. Для
     *  публичных данных безвреден, для торговых ручек обязателен. */
    function authHeaders() {
      const t = window.Sso && window.Sso.token && window.Sso.token();
      return t ? { Authorization: 'Bearer ' + t } : {};
    }

    /** GET с разбором JSON. Возвращает данные либо null (ошибка сети / не-2xx). */
    async function read(url) {
      try {
        const r = await fetch(url, { headers: authHeaders() });
        if (!r.ok) return null;
        return await r.json();
      } catch {
        return null;
      }
    }

    /** POST/DELETE с телом. Возвращает { ok, status, data }; data может быть null. */
    async function mutate(url, method, body) {
      const opts = { method, headers: authHeaders() };
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
      account: () => read('/account'),

      // ---- Позиции ---------------------------------------------------------
      deals: () => read('/deals'),
      openDeal: (body) => mutate('/deals', 'POST', body),
      closeDeal: (id) => mutate(`/deals/${id}/close`, 'POST'),
      closedDeals: () => read('/deals/closed'),

      // ---- Отложенные (лимитные) ордера -----------------------------------
      pending: () => read('/pending'),
      placePending: (body) => mutate('/pending', 'POST', body),
      cancelPending: (id) => mutate(`/pending/${id}`, 'DELETE'),
    };
  }

  window.Api = { create };
})();
