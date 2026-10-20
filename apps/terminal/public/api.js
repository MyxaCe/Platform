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

    /**
     * GET с разбором JSON и **названной причиной отказа** (КОНТРАКТЫ §4г).
     *
     * Возвращает `{ data, reason }`; `reason === null` — успех.
     *
     * Имена — ось канала, общая для всех потребителей платформы:
     * `network` (наша сеть не донесла), `http-error` (сосед ответил отказом),
     * `malformed` (ответил не тем). `not-configured` называет вызывающий: он
     * знает, задан ли адрес, а сюда без адреса не приходят.
     *
     * Почему `network` и `http-error` разведены: первое чинит наша сторона,
     * второе — сосед. Схлопнутые, они отправляют чинить наугад.
     *
     * Прежний `read` возвращал `null` на все три случая сразу — и это был не
     * косметический дефект: виджеты, умеющие различать причины, получали от
     * него одно `null` и показывали «ответ не тот» при оборванной сети.
     * Различие, построенное выше по течению, здесь и терялось.
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

    /** Короткая форма для вызывающих, которым причина не нужна. */
    async function read(url) {
      return (await readDetailed(url)).data;
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

    // Почему торговых данных нет. Отдельно от самих данных, потому что `null` не
    // отличает «сессии нет» от «сеть моргнула», а показывать эти два случая надо
    // по-разному: первый — устойчивый отказ, второй — повод оставить прежние цифры
    // (BUG-011: отказ не должен выглядеть правдоподобным значением).
    //   'unknown'     — успешного чтения ещё не было;
    //   'ok'          — данные пришли;
    //   'no-session'  — 401/403 или сессия не выдана: торговля недоступна;
    //   'unreachable' — сеть или не-2xx: связи нет, прежние данные ещё могут быть верны.
    let status = 'unknown';

    /** Чтение торговой ручки: возвращает данные либо null, попутно ставя `status`. */
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
      // ---- Состояние торгового доступа ------------------------------------
      /** 'unknown' | 'ok' | 'no-session' | 'unreachable' — см. `status` выше. */
      tradingStatus: () => status,

      // ---- Публичные данные (без авторизации) ----------------------------
      /** Состояние сервиса, в т.ч. режим хранилища: { storage, durable, sso }. */
      health: () => read('/health'),
      /** Список инструментов. `site` — фильтр витрины по allow-list сайта (ADR-023 Т3). */
      instruments: (site) => read('/instruments' + (site ? `?site=${encodeURIComponent(site)}` : '')),
      candles: (id, tf, limit) => read(`/candles/${id}?tf=${tf}&limit=${limit}`),
      depth: (id, limit) => read(`/depth/${id}${limit ? `?limit=${limit}` : ''}`),
      stats: (id) => read(`/stats/${id}`),

      // Те же ручки с названной причиной отказа — для виджетов, которые
      // показывают метку устаревания и обязаны сказать, что именно сломалось.
      depthDetailed: (id, limit) => readDetailed(`/depth/${id}${limit ? `?limit=${limit}` : ''}`),
      statsDetailed: (id) => readDetailed(`/stats/${id}`),

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
