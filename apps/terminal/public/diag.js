'use strict';
(function () {
  /** @type {Record<string, { badge: string, why: string }>} */
  const CHANNEL = {
    'not-configured': { badge: 'НЕ НАСТРОЕНО', why: 'адрес источника не задан' },
    network: { badge: 'НЕТ СВЯЗИ', why: 'источник не ответил' },
    'http-error': { badge: 'ОТКАЗ ИСТОЧНИКА', why: 'источник ответил отказом' },
    malformed: { badge: 'ОТВЕТ НЕ ТОТ', why: 'источник ответил не тем' },
    empty: { badge: 'ПУСТО', why: 'источник ответил пустым' },
  };

  /**
   * @param {string} reason
   * @returns {{ badge: string, why: string }}
   */
  function describe(reason) {
    const found = CHANNEL[reason];
    if (!found) {
      throw new Error(
        `[diag] причина «${reason}» не из словаря оси канала. ` +
        `Допустимые: ${Object.keys(CHANNEL).join(', ')}. ` +
        'Новое имя добавляется в КОНТРАКТЫ.md §4г, а не в код.',
      );
    }
    return found;
  }

  /** @param {{ subject?: string, onError?: (e: { reason: string, detail: any, count: number, first: boolean }) => void, every?: number, log?: { error: Function, info: Function } }} [cfg] */
  function reporter(cfg) {
    const subject = (cfg && cfg.subject) || 'diag';
    const onError = cfg && cfg.onError;
    const every = cfg && cfg.every;
    const log = (cfg && cfg.log) || console;

    /** @type {string|null} */
    let kind = null;
    let count = 0;

    return {
      state: () => kind,
      count: () => count,
      describe,

      /**
       * @param {string} reason
       * @param {any} [detail]
       */
      fail(reason, detail) {
        const info = describe(reason);
        const changed = kind !== reason;
        count = changed ? 1 : count + 1;
        kind = reason;

        const due = changed || (every ? count % every === 0 : false);
        if (due) {
          log.error(`[${subject}] ${info.why} (${reason}), подряд: ${count}`, detail);
        }
        if (onError) onError({ reason, detail, count, first: changed });

        return { badge: info.badge, why: info.why, first: changed };
      },

      ok() {
        if (kind) {
          log.info(`[${subject}] обновление восстановлено после ${count} неудач (${kind})`);
        }
        const was = kind;
        kind = null;
        count = 0;
        return was;
      },
    };
  }

  window.Diag = { reporter, describe, CHANNEL };
})();
