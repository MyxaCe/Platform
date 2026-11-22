(function () {
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

        let answer;
        try {
          answer = await ctx.fetch(id);
        } catch (e) {
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

        if (!Array.isArray(got.data)) {
          fail('malformed', got.data);
          return;
        }

        const list = /** @type {Array<{ tf: number|string, change: number|null }>} */ (got.data);
        succeeded();
        const change = ctx.change || ((/** @type {number} */ v) => `${v >= 0 ? '+' : ''}${v.toFixed(2)}%`);
        root.innerHTML = list.map((st) => {
          const label = (ctx.labels && ctx.labels[st.tf]) || st.tf;
          if (st.change == null) {
            return `<div class="stat"><span class="stat__l">${label}</span><span class="stat__v" title="no data">&mdash;</span></div>`;
          }
          const up = st.change >= 0;
          return `<div class="stat"><span class="stat__l">${label}</span><span class="stat__v ${up ? 'up' : 'down'}">${change(st.change)}</span></div>`;
        }).join('');
      }
      refresh();
      const timer = ctx.interval ? setInterval(refresh, ctx.interval) : null;
      return { refresh, destroy() { if (timer) clearInterval(timer); root.innerHTML = ''; } };
    },
  };
})();
