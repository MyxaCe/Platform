'use strict';
/* ============================================================================
 * rejection.js — отказ границы, объяснённый словами (TRM-43, КОНТРАКТЫ §4б).
 *
 * ЗАЧЕМ ОТДЕЛЬНЫМ ФАЙЛОМ. Функция жила внутри `app.js`, то есть внутри
 * монолита страницы, и проверить её было нечем: `app.js` трогает DOM с первой
 * строки и в Node не поднимается. Непроверяемая таблица текстов — это таблица,
 * в которой недостающая строка обнаруживается пользователем. Так и вышло: кода
 * `tenant_instruments_unreadable` здесь не было вовсе, человек видел
 * «Rejected 503: tenant_instruments_unreadable», и 24.10 эта строка отправила
 * наугад команду развёртывания — чинить то, что чинится правкой списка в CMS.
 *
 * ПРАВИЛО, КОТОРОЕ ЗДЕСЬ ИСПОЛНЯЕТСЯ. Отказ различает причину, и причины
 * чинятся по-разному (§4б). Их **три**, а не две, и третья — та самая, что
 * потерялась:
 *
 *   403 instrument_not_allowed        список говорит «нет» — окончательно;
 *   503 tenant_instruments_unavailable  списка НЕТ: источник не ответил
 *                                       → чинит развёртывание;
 *   503 tenant_instruments_unreadable   список ЕСТЬ, но в нём незнакомый
 *                                       символ → чинит ВЛАДЕЛЕЦ в CMS.
 *
 * Два последних — оба `503` и оба «торговля закрыта», поэтому их и склеивали.
 * Но один означает «связи нет», а другой — «опечатка в списке», и человек,
 * которому показали один код на оба случая, идёт чинить наугад.
 *
 * ЗАПАСНАЯ ВЕТКА НАЗЫВАЕТ СЕБЯ ЗАПАСНОЙ. Неизвестный код не притворяется
 * объяснением: он так и выводится — с пометкой, что терминал этого кода не
 * знает. Фраза на человеческом языке, выдуманная под незнакомый код, была бы
 * отказом, похожим на объяснение.
 * ========================================================================== */
(function () {
  /** Коды, у которых есть объяснение. Таблица, а не цепочка `if`: её видно. */
  const TEXT = {
    instrument_not_allowed: 'This instrument is not available on this site',
    tenant_instruments_unavailable:
      'Instrument permissions did not arrive — trading is closed. The site configuration is unreachable; this is fixed on the platform side.',
    tenant_instruments_unreadable:
      'The site instrument list contains an unknown symbol — trading is closed. The list itself is reachable; it needs correcting in the CMS.',
    insufficient_margin: 'Not enough free margin',
    // TRM-57. Два кода, а не один: человек правит РАЗНЫЕ поля и в разные
    // стороны, и «уровень задан неверно» на оба случая отправило бы его
    // проверять оба. Текст называет поле и сторону, потому что действие
    // именно в этом: сдвинуть уровень за текущую цену, а не передумать.
    stop_loss_already_reached:
      'Your stop loss is already on the other side of the price — the position would close immediately. Move it below the current price for a buy, above it for a sell.',
    take_profit_already_reached:
      'Your take profit is already reached — the position would close immediately. Move it above the current price for a buy, below it for a sell.',
    // Р-293. Счёт заводится первой операцией, и сумму для него называет САЙТ.
    // Три кода, потому что чинят их три разных человека: владелец в админке,
    // команда развёртывания, снова развёртывание. Один текст на всех отправил
    // бы человека наугад — ровно то, что случилось 24.10 с кодами списка.
    demo_start_balance_not_set:
      'This site has not set a starting balance for demo accounts, so your account was not opened. Nothing is wrong with your request — the site owner sets this amount in the admin panel.',
    demo_start_balance_unavailable:
      'Site settings could not be reached, so your account was not opened. Your request was fine; this is a platform-side outage and is being fixed there.',
    identity_store_unavailable:
      'We could not confirm which site your account belongs to, so it was not opened. Nothing is wrong with your request — this is a platform-side outage.',
  };

  // НЕ закрыты здесь намеренно, и названы, чтобы следующий не считал их
  // забытыми: `account_quarantined` (423) и `invariant_violated` (422).
  // Это отказы по состоянию счёта (КОНТРАКТЫ §4в), и формулировка для человека
  // у них — решение штаба, а не наше: она говорит клиенту о его деньгах.
  // До решения они попадают в запасную ветку, которая честно сообщает, что
  // терминал этой причины не знает, — а не выдумывает ему объяснение.

  /**
   * Текст отказа для человека.
   *
   * @param {number} status код ответа
   * @param {string} [code] машинная причина из тела ответа
   * @returns {string}
   */
  function text(status, code) {
    if (code && Object.prototype.hasOwnProperty.call(TEXT, code)) {
      return TEXT[code];
    }
    // Сессии нет — это про вход, а не про конкретную причину, и код сюда не
    // приходит вовсе. Поэтому ветка по статусу, и она НИЖЕ таблицы: код
    // точнее статуса, и затирать его статусом нельзя.
    if (status === 401) return 'No cabinet session — trading unavailable';
    return code
      ? `Rejected (${status}): terminal does not recognise the reason "${code}"`
      : `Rejected (${status})`;
  }

  window.Rejection = { text, TEXT };
})();
