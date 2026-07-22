/* ============================================================================
 * accounts.js — пошаговый вход (ADR-019).
 *
 *   почта → (сервер говорит, знаком ли адрес) → пароль → код из письма → готово
 *
 * Быстрые способы входа (Google, Apple, Telegram, MAX, Passkey, QR) описаны в
 * PROVIDERS. Каждый выключен, пока не настроен: кнопка, которая ничего не делает,
 * читается как поломка сервиса, поэтому она честно неактивна.
 * ========================================================================== */
(function () {
  const $ = (s) => document.querySelector(s);
  const t = I18n.t;
  /** Соседний поддомен того же окружения: accounts.localhost:8888 → my.localhost:8888 */
  const host = (sub) => {
    const base = location.hostname.replace(/^accounts\./, '');
    const port = location.port ? ':' + location.port : '';
    return `${location.protocol}//${sub}${base}${port}/`;
  };

  // Быстрые способы входа. `ready` включится, когда появятся ключи провайдера и домен.
  const PROVIDERS = [
    { id: 'google', label: 'Google', ready: false, mark: 'G' },
    { id: 'apple', label: 'Apple', ready: false, mark: 'A' },
    { id: 'telegram', label: 'Telegram', ready: false, mark: 'TG' },
    { id: 'max', label: 'MAX', ready: false, mark: 'M' },
    { id: 'qr', label: 'QR', ready: false, mark: 'QR' },
  ];

  let email = '';
  let isNew = false;
  let hasPasskey = false;

  const show = (name) => document.querySelectorAll('.step').forEach((s) => s.classList.toggle('hidden', s.dataset.step !== name));
  const setErr = (id, key) => { const e = $(id); e.textContent = key ? t(key) : ''; e.classList.toggle('on', !!key); };
  const post = async (url, body) => {
    try {
      const r = await fetch(url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body || {}) });
      return { status: r.status, ok: r.ok, data: await r.json().catch(() => null) };
    } catch { return { status: 0, ok: false, data: null }; }
  };
  const errKey = (s) => (s === 429 ? 'e.rate' : s === 401 ? 'e.creds' : 'e.net');

  // ---- Шаг 1: почта --------------------------------------------------------
  $('#providers').innerHTML = PROVIDERS.map((p) =>
    `<button class="prov" data-p="${p.id}"${p.ready ? '' : ' disabled'}><i>${p.mark}</i>${p.label}</button>`).join('');



  $('#formEmail').addEventListener('submit', async (e) => {
    e.preventDefault();
    setErr('#errEmail', '');
    const v = $('#email').value.trim();
    if (!/^[^\s@]+@[^\s@]+\.[^\s@]{2,}$/.test(v)) { setErr('#errEmail', 'e.email'); return; }
    const r = await post('/auth/check-email', { email: v });
    if (!r.ok) { setErr('#errEmail', errKey(r.status)); return; }
    email = v.toLowerCase();
    isNew = !r.data.registered;
    hasPasskey = !!r.data.has_passkey;
    // Второй шаг перерисовывается под ситуацию: вход или создание аккаунта.
    $('#pwTitle').textContent = t(isNew ? 's2.titleNew' : 's2.title');
    $('#pwEmail').textContent = `${t(isNew ? 's2.subNew' : 's2.subLogin')} ${email}`;
    $('#confirmWrap').classList.toggle('hidden', !isNew);
    $('#agreeWrap').classList.toggle('hidden', !isNew);
    // Вход по ключу предлагаем только тем, у кого он привязан, и только на входе.
    $('#usePasskey').classList.toggle('hidden', !(hasPasskey && !isNew && window.Passkey && Passkey.supported()));
    $('.agree').innerHTML = t('f.agree')
      .replace('{terms}', `<a href="${host('')}#more" target="_blank" rel="noopener">${t('f.terms')}</a>`)
      .replace('{risk}', `<a href="${host('')}#more" target="_blank" rel="noopener">${t('f.risk')}</a>`);
    show('password');
    $('#password').focus();
  });

  $('#usePasskey').addEventListener('click', async () => {
    const btn = $('#usePasskey'); btn.disabled = true;
    const r = await Passkey.login(email);
    btn.disabled = false;
    if (r.ok) { finish(); return; }
    if (r.error === 'unsupported' || r.error === 'NotAllowedError') return; // отмена — молча
    setErr('#errPass', r.status === 404 ? 'e.pknone' : 'e.net');
  });
  $('#backToEmail').addEventListener('click', () => { show('email'); $('#email').focus(); });
  $('#peek').addEventListener('click', () => {
    const i = $('#password');
    i.type = i.type === 'password' ? 'text' : 'password';
  });

  // ---- Шаг 2: пароль -------------------------------------------------------
  $('#formPass').addEventListener('submit', async (e) => {
    e.preventDefault();
    ['#errPass', '#errConfirm', '#errAgree'].forEach((id) => setErr(id, ''));
    const pass = $('#password').value;
    if (isNew) {
      if (pass.length < 8 || !/[a-zA-Z]/.test(pass) || !/\d/.test(pass)) { setErr('#errPass', 'e.pass'); return; }
      if ($('#confirm').value !== pass) { setErr('#errConfirm', 'e.confirm'); return; }
      if (!$('#agree').checked) { setErr('#errAgree', 'e.agree'); return; }
    } else if (!pass) {
      setErr('#errPass', 'e.pass');
      return;
    }
    const r = await post(isNew ? '/auth/register' : '/auth/login', { email, password: pass });
    if (!r.ok) { setErr('#errPass', r.status === 400 ? 'e.pass' : errKey(r.status)); return; }

    // Кто уже подтверждал почту, к коду не возвращается.
    if (!isNew && r.data && r.data.verified) { finish(); return; }
    await requestCode();
  });

  // ---- Шаг 3: код из письма ------------------------------------------------
  async function requestCode() {
    const r = await post('/auth/send-code');
    // Если код не запросился, шаг не меняем: иначе человек ждёт письмо, которого нет.
    if (!r.ok) { setErr('#errPass', r.status === 429 ? 'e.rate' : 'e.net'); return; }
    $('#codeSub').textContent = `${t('s3.sub')} ${email}`;
    // Почтовый сервер не настроен — говорим об этом прямо, а не делаем вид,
    // что письмо в пути.
    const undelivered = r.ok && r.data && r.data.delivered === false;
    $('#devNote').textContent = undelivered ? t('dev.note') : '';
    $('#devNote').classList.toggle('hidden', !undelivered);
    show('code');
    $('#code').focus();
  }
  $('#resend').addEventListener('click', requestCode);

  $('#formCode').addEventListener('submit', async (e) => {
    e.preventDefault();
    setErr('#errCode', '');
    const r = await post('/auth/verify-code', { code: $('#code').value.trim() });
    if (!r.ok) { setErr('#errCode', r.status === 429 ? 'e.rate' : 'e.code'); return; }
    finish();
  });

  function finish() {
    $('#toCabinet').href = host('my.');
    $('#toTerminal').href = host('trade.');
    show('done');
  }

  // ---- Язык, тема, ссылки --------------------------------------------------
  $('#toSite').href = host('');
  const paintLang = () => { $('#langBtn').textContent = I18n.lang().toUpperCase(); };
  $('#langBtn').addEventListener('click', () => I18n.setLang(I18n.lang() === 'en' ? 'ru' : 'en'));
  $('#themeBtn').addEventListener('click', () => Theme.toggle());
  I18n.onChange(paintLang);
  paintLang();
})();
