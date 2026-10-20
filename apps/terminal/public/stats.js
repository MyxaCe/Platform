/* ============================================================================
 * stats.js — переиспользуемый виджет статистики актива по периодам (ADR-015).
 * Не зависит от страницы: mount(el, ctx) строит ячейки «период → изменение %».
 *   const stats = AssetStats.mount(el, {
 *     instrument: () => currentId,
 *     fetch: (id) => Promise<[{ tf, change }]>,  // change в процентах
 *     labels: { 60: '1m', ... },                 // подписи периодов (опц.)
 *     interval: 5000,                            // опц.
 *   });
 * Возвращает { refresh(), destroy() }.
 * ========================================================================== */
(function () {
  // Стили компонента едут вместе с модулем (ADR-015): страница-хост может не иметь своей CSS.
  // Токены темы (--panel/--muted/--up/--down) — контракт хоста; у каждого задан фолбэк,
  // поэтому виджет отрисуется корректно и на странице без темы.
  const CSS = `
.statbar { display: flex; height: 30px; flex: 0 0 30px; align-items: stretch; border-top: 1px solid var(--border,#1f2735); background: var(--panel,#10141d); }
.stat { flex: 1; display: flex; align-items: center; justify-content: center; gap: 6px; border-right: 1px solid rgba(31,39,53,.5); font-size: 11px; }
.stat:last-child { border-right: none; }
.stat__l { color: var(--muted,#6b7688); text-transform: uppercase; }
.stat__v { font-variant-numeric: tabular-nums; font-weight: 600; }
.stat__v.up { color: var(--up,#26a69a); } .stat__v.down { color: var(--down,#ef5350); }

/* Полоса, которая перестала обновляться (TRM-16). Числа остаются — они были
   верны на момент последнего успешного чтения, — но перестают выглядеть
   свежими, и рядом стоит причина. Отказ обязан быть виден как отказ. */
.statbar[data-stale] { position: relative; }
.statbar[data-stale] .stat__v, .statbar[data-stale] .stat__l { opacity: .4; }
.statbar[data-stale]::after { content: attr(data-stale-label); position: absolute; right: 8px; top: 50%;
  transform: translateY(-50%); font-size: 10px; font-weight: 700; letter-spacing: .4px;
  color: var(--down,#ef5350); background: var(--panel,#10141d); padding: 1px 6px; border-radius: 4px;
  border: 1px solid var(--down,#ef5350); pointer-events: none; }
`;
  if (!document.getElementById('assetstats-css')) {
    const s = document.createElement('style'); s.id = 'assetstats-css'; s.textContent = CSS;
    document.head.appendChild(s);
  }

  window.AssetStats = {
    /**
     * @param {HTMLElement} root
     * @param {any} ctx
     */
    mount(root, ctx) {
      root.classList.add('statbar');

      /**
       * Отказ обязан быть виден как отказ (TRM-16).
       *
       * Было `catch { return; }`: сетевой блип и ответ не той формы
       * неразличимы, и наружу не выходило ни одного из них. Полоса продолжала
       * показывать старые числа как свежие — то есть молчаливый отказ надевал
       * вид работающего виджета.
       *
       * Теперь у отказа три следа, и каждый для своего читателя:
       *   - на экране — метка и погасшие числа: смотрящий видит, что полоса
       *     устарела, но прежние значения не стёрты (они были верны на момент
       *     последнего успешного чтения, и это честнее пустоты);
       *   - в консоли — строка с РАЗЛИЧЁННОЙ причиной: `unreachable` (источник
       *     не ответил) против `malformed` (ответил не тем). Чинятся они
       *     по-разному, и схлопывать их в один отказ значит отправлять
       *     чинить наугад;
       *   - наружу — `ctx.onError`, чтобы хост мог поднять свой индикатор.
       *
       * В консоль пишем на СМЕНЕ состояния, а не на каждом тике: опрос идёт
       * каждые несколько секунд, и поток одинаковых строк читать перестанут
       * так же быстро, как молчание. Повторы считаются и называются числом при
       * восстановлении.
       */
      /**
       * Диагностика — общим помощником (TRM-17), а не своей политикой.
       *
       * Раньше здесь жили свои имена причин (`unreachable`/`malformed`) и своя
       * политика вывода. Имена оказались неверны: `unreachable` склеивал нашу
       * сеть и отказ соседа — разные неполадки у разных людей. Теперь имена из
       * `КОНТРАКТЫ.md` §4г, и приходят они от `api.js`, который единственный
       * знает, что именно не получилось.
       */
      const diag = window.Diag.reporter({ subject: 'stats', onError: ctx.onError, log: ctx.log });

      /**
       * @param {string} reason
       * @param {any} [detail]
       */
      function fail(reason, detail) {
        const { badge, why } = diag.fail(reason, detail);
        root.setAttribute('data-stale', reason);
        root.setAttribute('data-stale-label', badge);
        root.title = `Статистика не обновляется: ${why}. Числа — с последнего успешного чтения.`;
      }

      function succeeded() {
        diag.ok();
        root.removeAttribute('data-stale');
        root.removeAttribute('data-stale-label');
        root.title = '';
      }

      async function refresh() {
        const id = ctx.instrument(); if (id == null) return;

        // Источник отдаёт `{ data, reason }`: причину называет тот, кто знает,
        // что именно не получилось, а не тот, кто получил пустоту.
        let answer;
        try {
          answer = await ctx.fetch(id);
        } catch (e) {
          // Исключение из слоя чтения — это «не донесли», то есть `network`.
          // Глушить его, как раньше, нельзя: отказ обязан быть виден.
          fail('network', e);
          return;
        }

        const got = answer && typeof answer === 'object' && 'reason' in answer
          ? answer
          : { data: answer, reason: null };

        if (got.reason) {
          fail(got.reason, got.status);
          return;
        }

        // Форма ответа — наша зона: канал донёс, но не то. Это `malformed` той
        // же оси, и слово здесь то же самое.
        if (!Array.isArray(got.data)) {
          fail('malformed', got.data);
          return;
        }

        const list = /** @type {Array<{ tf: number|string, change: number|null }>} */ (got.data);
        succeeded();
        root.innerHTML = list.map((st) => {
          const label = (ctx.labels && ctx.labels[st.tf]) || st.tf;
          // change === null — сервер не смог посчитать. Раньше он присылал в этом
          // случае 0, и «не смогли» рисовалось как «рынок стоял» (BUG-011).
          if (st.change == null) {
            return `<div class="stat"><span class="stat__l">${label}</span><span class="stat__v" title="no data">&mdash;</span></div>`;
          }
          const up = st.change >= 0;
          return `<div class="stat"><span class="stat__l">${label}</span><span class="stat__v ${up ? 'up' : 'down'}">${up ? '+' : ''}${st.change.toFixed(2)}%</span></div>`;
        }).join('');
      }
      refresh(); // первичная отрисовка сразу при монтировании — виджет ставится одной строкой
      const timer = ctx.interval ? setInterval(refresh, ctx.interval) : null;
      return { refresh, destroy() { if (timer) clearInterval(timer); root.innerHTML = ''; } };
    },
  };
})();
