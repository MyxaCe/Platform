/* ============================================================================
 * passkey.js — браузерная часть Passkey/WebAuthn (ADR-020). ОБЩИЙ модуль:
 * accounts вызывает login(), cabinet — register().
 *
 * Всю криптографию делают браузер (navigator.credentials) и сервер (webauthn-rs).
 * Здесь только перегон данных: сервер шлёт challenge и id в base64url, а WebAuthn
 * хочет ArrayBuffer; ответ устройства — наоборот. Плюс единый разбор ошибок.
 * ========================================================================== */
(function () {
  const supported = () => !!(window.PublicKeyCredential && navigator.credentials && navigator.credentials.create);

  // base64url ↔ ArrayBuffer. Значение может прийти строкой (base64url) или, у
  // некоторых версий сервера, массивом байт — принимаем оба.
  const toBuf = (v) => {
    if (v instanceof ArrayBuffer) return v;
    if (Array.isArray(v)) return new Uint8Array(v).buffer;
    let s = String(v).replace(/-/g, '+').replace(/_/g, '/');
    s += '='.repeat((4 - (s.length % 4)) % 4);
    const bin = atob(s);
    const u = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i);
    return u.buffer;
  };
  const b64u = (buf) => {
    const u = new Uint8Array(buf);
    let s = '';
    for (const b of u) s += String.fromCharCode(b);
    return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  };

  const post = (url, body) => fetch(url, {
    method: 'POST',
    headers: body ? { 'content-type': 'application/json' } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });

  /** Привязать новый ключ к уже вошедшему пользователю. Требует сессии. */
  async function register() {
    if (!supported()) return { ok: false, error: 'unsupported' };
    const r1 = await post('/auth/passkey/register/start');
    if (!r1.ok) return { ok: false, status: r1.status };
    const { flow, options } = await r1.json();
    const pk = options.publicKey;
    pk.challenge = toBuf(pk.challenge);
    pk.user.id = toBuf(pk.user.id);
    (pk.excludeCredentials || []).forEach((c) => { c.id = toBuf(c.id); });

    let cred;
    try { cred = await navigator.credentials.create({ publicKey: pk }); }
    catch (e) { return { ok: false, error: e.name || 'aborted' }; }

    const r2 = await post('/auth/passkey/register/finish', {
      flow,
      credential: {
        id: cred.id, rawId: b64u(cred.rawId), type: cred.type,
        response: {
          attestationObject: b64u(cred.response.attestationObject),
          clientDataJSON: b64u(cred.response.clientDataJSON),
        },
      },
    });
    return { ok: r2.ok, status: r2.status };
  }

  /** Войти существующим ключом. Почта известна с шага 1 — сервер отдаёт список
   *  ключей этого пользователя (allowCredentials), браузер выбирает подходящий. */
  async function login(email) {
    if (!supported()) return { ok: false, error: 'unsupported' };
    const r1 = await post('/auth/passkey/login/start', { email });
    if (!r1.ok) return { ok: false, status: r1.status };
    const { flow, options } = await r1.json();
    const pk = options.publicKey;
    pk.challenge = toBuf(pk.challenge);
    (pk.allowCredentials || []).forEach((c) => { c.id = toBuf(c.id); });

    let a;
    try { a = await navigator.credentials.get({ publicKey: pk }); }
    catch (e) { return { ok: false, error: e.name || 'aborted' }; }

    const r2 = await post('/auth/passkey/login/finish', {
      flow,
      credential: {
        id: a.id, rawId: b64u(a.rawId), type: a.type,
        response: {
          authenticatorData: b64u(a.response.authenticatorData),
          clientDataJSON: b64u(a.response.clientDataJSON),
          signature: b64u(a.response.signature),
          userHandle: a.response.userHandle ? b64u(a.response.userHandle) : null,
        },
      },
    });
    return { ok: r2.ok, status: r2.status, data: r2.ok ? await r2.json() : null };
  }

  window.Passkey = { supported, register, login };
})();
