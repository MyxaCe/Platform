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
  window.AssetStats = {
    mount(root, ctx) {
      async function refresh() {
        const id = ctx.instrument(); if (id == null) return;
        let list; try { list = await ctx.fetch(id); } catch { return; }
        if (!Array.isArray(list)) return;
        root.innerHTML = list.map((st) => {
          const up = st.change >= 0;
          const label = (ctx.labels && ctx.labels[st.tf]) || st.tf;
          return `<div class="stat"><span class="stat__l">${label}</span><span class="stat__v ${up ? 'up' : 'down'}">${up ? '+' : ''}${st.change.toFixed(2)}%</span></div>`;
        }).join('');
      }
      const timer = ctx.interval ? setInterval(refresh, ctx.interval) : null;
      return { refresh, destroy() { if (timer) clearInterval(timer); root.innerHTML = ''; } };
    },
  };
})();
