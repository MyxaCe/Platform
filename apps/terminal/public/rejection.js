'use strict';
(function () {
  const TEXT = {
    instrument_not_allowed: 'This instrument is not available on this site',
    tenant_instruments_unavailable:
      'Instrument permissions did not arrive — trading is closed. The site configuration is unreachable; this is fixed on the platform side.',
    tenant_instruments_unreadable:
      'The site instrument list contains an unknown symbol — trading is closed. The list itself is reachable; it needs correcting in the CMS.',
    insufficient_margin: 'Not enough free margin',
    stop_loss_already_reached:
      'Your stop loss is already on the other side of the price — the position would close immediately. Move it below the current price for a buy, above it for a sell.',
    take_profit_already_reached:
      'Your take profit is already reached — the position would close immediately. Move it above the current price for a buy, below it for a sell.',
    stop_loss_too_close:
      'Your stop loss is closer to the price than the minimum distance set by the site. Move it further away: lower for a buy, higher for a sell.',
    take_profit_too_close:
      'Your take profit is closer to the price than the minimum distance set by the site. Move it further away: higher for a buy, lower for a sell.',
    demo_start_balance_not_set:
      'This site has not set a starting balance for demo accounts, so your account was not opened. Nothing is wrong with your request — the site owner sets this amount in the admin panel.',
    demo_start_balance_unavailable:
      'Site settings could not be reached, so your account was not opened. Your request was fine; this is a platform-side outage and is being fixed there.',
    account_frozen:
      'Your account is blocked by an operator: opening positions, new orders and withdrawals are disabled. You can still close existing positions. Please contact support.',
    identity_store_unavailable:
      'We could not confirm which site your account belongs to, so it was not opened. Nothing is wrong with your request — this is a platform-side outage.',
    sltp_edit_disabled:
      'Changing SL/TP of an open position or order is not available yet. To change a level now, close the position or cancel the order and place it again.',
    unknown_position:
      'This position is no longer open — it may have just been closed by its stop loss or take profit.',
    unknown_pending:
      'This order is no longer active — it may have just been filled or cancelled.',
    invalid_level: 'Enter a price above zero, or leave the field empty to remove the level.',
  };

  /**
   * @param {number} status
   * @param {string} [code]
   * @returns {string}
   */
  function text(status, code) {
    if (code && Object.prototype.hasOwnProperty.call(TEXT, code)) {
      return TEXT[code];
    }
    if (status === 401) return 'No cabinet session — trading unavailable';
    return code
      ? `Rejected (${status}): terminal does not recognise the reason "${code}"`
      : `Rejected (${status})`;
  }

  window.Rejection = { text, TEXT };
})();
