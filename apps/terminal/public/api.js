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

    // В embedded-режиме (задан origin кабинета, ADR-023) без сессии торговые ручки
    // отдают 401 — не долбим их и не шумим в консоли, пока не пришёл handoff-токен.
    // В standalone (origin кабинета не задан) — гоняем как есть (сервер на счёте по умолчанию).
    function tradingBlocked() {
      const embedded = ((window.TERMINAL_CONFIG || {}).cabinetOrigins || []).some(Boolean);
      return embedded && !(window.Sso && window.Sso.token());
    }
    const authedRead = (url) => (tradingBlocked() ? Promise.resolve(null) : read(url));
    const authedMutate = (url, method, body) =>
      (tradingBlocked() ? Promise.resolve({ ok: false, status: 401, data: null }) : mutate(url, method, body));

    return {
      // ---- Публичные данные (без авторизации) ----------------------------
      instruments: () => read('/instruments'),
      candles: (id, tf, limit) => read(`/candles/${id}?tf=${tf}&limit=${limit}`),
      depth: (id, limit) => read(`/depth/${id}${limit ? `?limit=${limit}` : ''}`),
      stats: (id) => read(`/stats/${id}`),

      // ---- Счёт пользователя (требует сессию) ----------------------------
      account: () => authedRead('/account'),

      // ---- Позиции (требуют сессию) ---------------------------------------
      deals: () => authedRead('/deals'),
      openDeal: (body) => authedMutate('/deals', 'POST', body),
      closeDeal: (id) => authedMutate(`/deals/${id}/close`, 'POST'),
      closedDeals: () => authedRead('/deals/closed'),

      // ---- Отложенные (лимитные) ордера (требуют сессию) ------------------
      pending: () => authedRead('/pending'),
      placePending: (body) => authedMutate('/pending', 'POST', body),
      cancelPending: (id) => authedMutate(`/pending/${id}`, 'DELETE'),
    };
  }

  window.Api = { create };
})();
