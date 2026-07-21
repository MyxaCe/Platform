/* ============================================================================
 * auth.js — окна входа и регистрации (контракт ADR-015: свой DOM, свои стили,
 * зависимости через ctx).
 *
 *   const auth = Auth.mount(document.body, {
 *     t: I18n.t,
 *     submit: async (mode, data) => ({ ok, message }),  // отправка — снаружи
 *     onSuccess: (mode, data) => {},
 *   });
 *   auth.open('login' | 'register');  auth.close();  auth.relabel();
 *
 * Отправку модуль не делает сам намеренно: сегодня настоящей системы аккаунтов
 * нет, а вешать публичную форму на dev-эндпоинт `/admin/users` (без пароля и без
 * защиты) значит дать кому угодно создавать пользователей. Появится нормальный
 * auth — меняется только `ctx.submit`.
 *
 * Доступность: role="dialog" + aria-modal, фокус запирается внутри окна и
 * возвращается на кнопку, которая его открыла; Esc и клик по фону закрывают.
 * ========================================================================== */
(function () {
  const CSS = `
.au { position: fixed; inset: 0; z-index: 100; display: flex; align-items: center; justify-content: center;
  padding: 20px; background: rgba(3, 6, 12, .62); backdrop-filter: blur(2px); }
.au.hidden { display: none; }
.au__card { width: 100%; max-width: 424px; max-height: calc(100vh - 40px); overflow-y: auto;
  background: var(--bg); border: 1px solid var(--line); border-radius: 14px; box-shadow: var(--shadow); padding: 28px; }
.au__top { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; }
.au__title { font-size: 22px; font-weight: 700; letter-spacing: -.02em; }
.au__sub { margin-top: 6px; color: var(--muted); font-size: 14px; }
.au__x { background: none; border: none; color: var(--muted); cursor: pointer; padding: 4px; border-radius: 6px; line-height: 0; }
.au__x:hover { background: var(--surface-2); color: var(--text); }
.au__x svg { width: 18px; height: 18px; }

.au__form { margin-top: 22px; display: flex; flex-direction: column; gap: 15px; }
.au__field label { display: block; font-size: 13px; font-weight: 600; margin-bottom: 6px; }
.au__ctrl { position: relative; display: flex; align-items: center; }
.au__field input[type=email], .au__field input[type=password], .au__field input[type=text] {
  width: 100%; height: 42px; padding: 0 12px; border-radius: 9px; border: 1px solid var(--line);
  background: var(--surface); color: var(--text); font: inherit; font-size: 14.5px; outline: none;
  transition: border-color .15s, box-shadow .15s;
}
.au__field input:focus { border-color: var(--accent); box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 22%, transparent); }
.au__field.bad input { border-color: var(--down); }
.au__peek { position: absolute; right: 6px; background: none; border: none; color: var(--muted); cursor: pointer;
  padding: 6px; border-radius: 6px; line-height: 0; }
.au__peek:hover { color: var(--text); background: var(--surface-2); }
.au__peek svg { width: 17px; height: 17px; }
.au__err { display: none; margin-top: 6px; color: var(--down); font-size: 12.5px; }
.au__field.bad .au__err { display: block; }

.au__meter { display: flex; align-items: center; gap: 8px; margin-top: 8px; font-size: 12px; color: var(--muted); }
.au__meter i { flex: 1; height: 4px; border-radius: 2px; background: var(--surface-2); overflow: hidden; }
.au__meter i b { display: block; height: 100%; width: 0; background: var(--down); transition: width .2s, background .2s; }
.au__meter.m2 i b { background: #e0a800; } .au__meter.m3 i b { background: var(--up); }

.au__row { display: flex; align-items: center; justify-content: space-between; gap: 12px; font-size: 13.5px; }
.au__check { display: flex; align-items: flex-start; gap: 9px; font-size: 13.5px; cursor: pointer; line-height: 1.45; }
/* Специфичнее правила .au__field label выше: иначе строка согласия получает
   display:block, gap перестаёт работать и чекбокс липнет к тексту. */
.au__field label.au__check { display: flex; font-size: 13.5px; margin-bottom: 0; }
.au__check input { margin: 2px 0 0; width: 16px; height: 16px; accent-color: var(--accent); cursor: pointer; flex: 0 0 16px; }
.au__check a { color: var(--accent); text-decoration: underline; text-underline-offset: 2px; }
.au__link { color: var(--accent); font-size: 13.5px; }
.au__link:hover { text-decoration: underline; }

.au__submit { height: 44px; margin-top: 4px; border: none; border-radius: 9px; background: var(--accent);
  color: var(--accent-ink); font: inherit; font-size: 15px; font-weight: 700; cursor: pointer; }
.au__submit:hover { filter: brightness(1.07); }
.au__submit[disabled] { opacity: .6; cursor: default; }

.au__note { display: none; margin-top: 14px; padding: 11px 13px; border-radius: 9px; font-size: 13px;
  background: var(--surface-2); border: 1px solid var(--line); color: var(--muted); }
.au__note.show { display: block; }
.au__foot { margin-top: 18px; text-align: center; font-size: 13.5px; color: var(--muted); }
.au__foot button { background: none; border: none; color: var(--accent); font: inherit; font-weight: 600; cursor: pointer; padding: 0 0 0 4px; }
.au__foot button:hover { text-decoration: underline; }
`;
  if (!document.getElementById('auth-css')) {
    const s = document.createElement('style'); s.id = 'auth-css'; s.textContent = CSS;
    document.head.appendChild(s);
  }

  const EYE = '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M1.7 10S4.7 4.6 10 4.6 18.3 10 18.3 10 15.3 15.4 10 15.4 1.7 10 1.7 10Z"/><circle cx="10" cy="10" r="2.4"/></svg>';
  const EYE_OFF = '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M4 4l12 12"/><path d="M8.2 5c.6-.2 1.2-.3 1.8-.3 5.3 0 8.3 5.3 8.3 5.3a15 15 0 0 1-2.6 3.2M5.6 6.7A15 15 0 0 0 1.7 10S4.7 15.4 10 15.4c1 0 1.9-.2 2.7-.5"/></svg>';
  const X = '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6"><path d="M5 5l10 10M15 5L5 15"/></svg>';

  const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/;
  const passOk = (v) => v.length >= 8 && /[a-zA-Z]/.test(v) && /\d/.test(v);
  /** 0..3 — грубая оценка: длина + разнообразие символов. */
  function strength(v) {
    if (!v) return 0;
    let n = 0;
    if (v.length >= 8) n++;
    if (v.length >= 12) n++;
    if (/[a-zA-Z]/.test(v) && /\d/.test(v)) n++;
    if (/[^a-zA-Z0-9]/.test(v)) n++;
    return Math.min(3, n);
  }

  function mount(root, ctx) {
    const t = ctx.t || ((k) => k);
    let mode = 'login';
    let lastFocused = null;

    const el = document.createElement('div');
    el.className = 'au hidden';
    el.innerHTML = `
      <div class="au__card" role="dialog" aria-modal="true" aria-labelledby="auTitle">
        <div class="au__top">
          <div>
            <div class="au__title" id="auTitle"></div>
            <div class="au__sub"></div>
          </div>
          <button class="au__x" type="button">${X}</button>
        </div>
        <form class="au__form" novalidate>
          <div class="au__field" data-f="email">
            <label for="auEmail"></label>
            <div class="au__ctrl"><input id="auEmail" type="email" autocomplete="email" /></div>
            <div class="au__err"></div>
          </div>
          <div class="au__field" data-f="pass">
            <label for="auPass"></label>
            <div class="au__ctrl">
              <input id="auPass" type="password" />
              <button class="au__peek" type="button" data-for="auPass">${EYE}</button>
            </div>
            <div class="au__err"></div>
            <div class="au__meter"><span class="au__meter-l"></span><i><b></b></i><span class="au__meter-v"></span></div>
          </div>
          <div class="au__field" data-f="confirm">
            <label for="auConfirm"></label>
            <div class="au__ctrl">
              <input id="auConfirm" type="password" autocomplete="new-password" />
              <button class="au__peek" type="button" data-for="auConfirm">${EYE}</button>
            </div>
            <div class="au__err"></div>
          </div>
          <div class="au__row" data-f="remember">
            <label class="au__check"><input type="checkbox" id="auRemember" /><span></span></label>
            <a class="au__link" href="#more"></a>
          </div>
          <div class="au__field" data-f="agree">
            <label class="au__check"><input type="checkbox" id="auAgree" /><span class="au__agree"></span></label>
            <div class="au__err"></div>
          </div>
          <button class="au__submit" type="submit"></button>
        </form>
        <div class="au__note"></div>
        <div class="au__foot"><span class="au__foot-q"></span><button type="button" class="au__switch"></button></div>
      </div>`;
    root.appendChild(el);

    const $ = (s) => el.querySelector(s);
    const card = $('.au__card'), form = $('.au__form'), note = $('.au__note');
    const fields = { email: $('[data-f=email]'), pass: $('[data-f=pass]'), confirm: $('[data-f=confirm]'), remember: $('[data-f=remember]'), agree: $('[data-f=agree]') };
    const inEmail = $('#auEmail'), inPass = $('#auPass'), inConfirm = $('#auConfirm'), inAgree = $('#auAgree');
    const meter = $('.au__meter');

    /** Подписи и состав формы под текущий режим. */
    function relabel() {
      const reg = mode === 'register';
      $('.au__title').textContent = t(reg ? 'auth.register.t' : 'auth.login.t');
      $('.au__sub').textContent = t(reg ? 'auth.register.s' : 'auth.login.s');
      $('.au__x').setAttribute('aria-label', t('auth.close'));
      fields.email.querySelector('label').textContent = t('auth.email');
      fields.pass.querySelector('label').textContent = t('auth.password');
      fields.confirm.querySelector('label').textContent = t('auth.confirm');
      fields.remember.querySelector('.au__check span').textContent = t('auth.remember');
      fields.remember.querySelector('.au__link').textContent = t('auth.forgot');
      $('.au__meter-l').textContent = t('auth.strength');
      $('.au__submit').textContent = t(reg ? 'auth.do.register' : 'auth.do.login');
      $('.au__foot-q').textContent = t(reg ? 'auth.haveAccount' : 'auth.noAccount');
      $('.au__switch').textContent = t(reg ? 'auth.toLogin' : 'auth.toRegister');
      inPass.autocomplete = reg ? 'new-password' : 'current-password';

      // Согласие с условиями — с рабочими ссылками, поэтому собирается из шаблона.
      $('.au__agree').innerHTML = t('auth.agree')
        .replace('{terms}', `<a href="#more">${t('auth.terms')}</a>`)
        .replace('{risk}', `<a href="#more">${t('auth.risk')}</a>`);

      // Подтверждение пароля и согласие — только при регистрации; «запомнить» — только при входе.
      fields.confirm.classList.toggle('hidden', !reg);
      fields.agree.classList.toggle('hidden', !reg);
      meter.classList.toggle('hidden', !reg);
      fields.remember.classList.toggle('hidden', reg);
    }

    const setErr = (f, key) => { fields[f].classList.toggle('bad', !!key); if (key) fields[f].querySelector('.au__err').textContent = t(key); };
    const clearErrs = () => Object.keys(fields).forEach((f) => fields[f].classList.remove('bad'));

    function paintMeter() {
      const s = strength(inPass.value);
      meter.classList.remove('m1', 'm2', 'm3');
      if (s) meter.classList.add('m' + s);
      meter.querySelector('b').style.width = `${(s / 3) * 100}%`;
      meter.querySelector('.au__meter-v').textContent = s ? t(['', 'auth.weak', 'auth.medium', 'auth.strong'][s]) : '';
    }
    inPass.addEventListener('input', paintMeter);

    el.querySelectorAll('.au__peek').forEach((b) => b.addEventListener('click', () => {
      const inp = el.querySelector('#' + b.dataset.for);
      const shown = inp.type === 'text';
      inp.type = shown ? 'password' : 'text';
      b.innerHTML = shown ? EYE : EYE_OFF;
      b.setAttribute('aria-label', t(shown ? 'auth.show' : 'auth.hide'));
    }));

    function validate() {
      clearErrs();
      let ok = true;
      if (!EMAIL_RE.test(inEmail.value.trim())) { setErr('email', 'auth.err.email'); ok = false; }
      if (!passOk(inPass.value)) { setErr('pass', 'auth.err.pass'); ok = false; }
      if (mode === 'register') {
        if (inConfirm.value !== inPass.value || !inConfirm.value) { setErr('confirm', 'auth.err.confirm'); ok = false; }
        if (!inAgree.checked) { setErr('agree', 'auth.err.agree'); ok = false; }
      }
      return ok;
    }

    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      note.classList.remove('show');
      if (!validate()) return;
      const data = { email: inEmail.value.trim(), password: inPass.value };
      const btn = $('.au__submit'); btn.disabled = true;
      let res = { ok: false, message: t('auth.soon') };
      try { if (ctx.submit) res = await ctx.submit(mode, data); } catch { /* показываем сообщение ниже */ }
      btn.disabled = false;
      note.textContent = (res && res.message) || t('auth.soon');
      note.classList.add('show');
      if (res && res.ok && ctx.onSuccess) ctx.onSuccess(mode, data);
    });

    $('.au__switch').addEventListener('click', () => open(mode === 'login' ? 'register' : 'login'));
    $('.au__x').addEventListener('click', close);
    el.addEventListener('mousedown', (e) => { if (e.target === el) close(); });

    // Фокус не должен уходить из окна: Tab по кругу, Esc закрывает.
    el.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') { close(); return; }
      if (e.key !== 'Tab') return;
      const f = [...card.querySelectorAll('button, input, a[href]')].filter((n) => n.offsetParent !== null && !n.disabled);
      if (!f.length) return;
      const first = f[0], last = f[f.length - 1];
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
    });

    function open(next) {
      mode = next === 'register' ? 'register' : 'login';
      if (el.classList.contains('hidden')) lastFocused = document.activeElement;
      clearErrs(); note.classList.remove('show'); form.reset(); paintMeter();
      relabel();
      el.classList.remove('hidden');
      document.body.style.overflow = 'hidden';
      inEmail.focus();
    }
    function close() {
      el.classList.add('hidden');
      document.body.style.overflow = '';
      if (lastFocused && lastFocused.focus) lastFocused.focus();
    }

    relabel();
    return { open, close, relabel, destroy() { el.remove(); } };
  }

  window.Auth = { mount };
})();
