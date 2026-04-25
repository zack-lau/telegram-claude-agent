#!/usr/bin/env python3
"""
ML strategy premarket execution — IBKR paper account.

Thin broker adapter: loads scores, reconciles IBKR positions, delegates
all strategy logic to ml_strategy_core, executes via ib_insync.

Usage:
  source ~/trading/.venv/bin/activate
  python3 -u ~/trading/scripts/ml_premarket_ibkr.py [--dry-run]

State file: ~/trading/ml/portfolio_state/state_ibkr.json
Log:        ~/trading/logs/ml-premarket-ibkr.log
Runs daily at 6:15 AM ET via systemd ml-premarket-ibkr.timer
"""

import html
import math
import os
import sys
import time
from pathlib import Path

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------
IBKR_HOST = os.environ.get("IBKR_HOST", "127.0.0.1")
IBKR_PORT = int(os.environ.get("IBKR_PORT", "4002"))
IBKR_CLIENT_ID = int(os.environ.get("IBKR_CLIENT_ID", "13"))
IBKR_ACCOUNT = os.environ.get("IBKR_ACCOUNT")
if not IBKR_ACCOUNT:
    raise RuntimeError("IBKR_ACCOUNT env var required")

TG_BOT_TOKEN = os.environ.get("TG_BOT_TOKEN", "")
TG_CHAT_ID = os.environ.get("TG_CHAT_ID", "496920142")

STATE_DIR = Path.home() / "trading" / "ml" / "portfolio_state"
STATE_PATH = STATE_DIR / "state_ibkr.json"

ORDER_OFFSET_PCT = 0.003
STOP_LOSS_PCT = 0.075


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
def send_telegram(message: str):
    if not TG_BOT_TOKEN:
        return
    try:
        import requests
        url = f"https://api.telegram.org/bot{TG_BOT_TOKEN}/sendMessage"
        payload = {"chat_id": TG_CHAT_ID, "text": message, "parse_mode": "HTML"}
        resp = requests.post(url, json=payload, timeout=10)
        if not resp.ok:
            print(f"  Telegram send failed: {resp.status_code} {resp.text[:100]}", flush=True)
    except Exception as e:
        print(f"  Telegram send failed: {e}", flush=True)


def connect_ibkr():
    from ib_insync import IB
    ib = IB()
    ib.connect(IBKR_HOST, IBKR_PORT, clientId=IBKR_CLIENT_ID, timeout=30)
    print(f"  Connected to IBKR: {ib.managedAccounts()}", flush=True)
    return ib


def check_ibkr_connected(ib) -> bool:
    """Return True if the IB Gateway connection is alive.

    Uses ib.isConnected() (socket-level) and a lightweight reqCurrentTime()
    round-trip.  Either failure means the gateway dropped mid-session.
    """
    import logging
    logger = logging.getLogger(__name__)
    if not ib.isConnected():
        logger.error("IB Gateway connection lost (isConnected=False)")
        return False
    try:
        ib.reqCurrentTime()
        return True
    except Exception as exc:
        logger.error(f"IB Gateway ping failed: {exc}")
        return False


def reconcile_positions(ib, state: dict, capital: float) -> dict:
    from ib_insync import Stock
    import ml_strategy_core as core
    positions = ib.positions()
    actual = {}
    pos_by_sym = {}
    for pos in positions:
        if pos.account == IBKR_ACCOUNT and isinstance(pos.contract, Stock):
            actual[pos.contract.symbol] = pos.position
            pos_by_sym[pos.contract.symbol] = pos

    state_longs = dict(state.get("current_longs", {}))
    state_shorts = dict(state.get("current_shorts", {}))
    reconciled_longs = {}
    reconciled_shorts = {}
    discrepancies = []

    for sym in sorted(set(state_longs) | set(state_shorts) | set(actual)):
        qty = actual.get(sym, 0)
        if qty > 0:
            if sym in state_longs:
                reconciled_longs[sym] = state_longs[sym]
            else:
                pos = pos_by_sym.get(sym)
                mv = getattr(pos, 'marketValue', None)
                market_val = abs(mv) if mv is not None else (abs(pos.position) * pos.avgCost if pos else 0)
                broker_weight = market_val / capital if (capital > 0 and market_val > 0) else 0
                if broker_weight > 0:
                    reconciled_longs[sym] = broker_weight
                    if abs(broker_weight - 1.0 / core.NAMES_PER_SIDE) / (1.0 / core.NAMES_PER_SIDE) > 0.5:
                        import logging
                        logging.getLogger(__name__).warning(
                            f"New position {sym}: broker weight {broker_weight:.3f} differs from default {1.0 / core.NAMES_PER_SIDE:.3f}")
                else:
                    reconciled_longs[sym] = 1.0 / core.NAMES_PER_SIDE
                discrepancies.append(f"  {sym}: IBKR=long({qty}), state=none → added (weight={reconciled_longs[sym]:.3f})")
        elif qty < 0:
            if sym in state_shorts:
                reconciled_shorts[sym] = state_shorts[sym]
            else:
                pos = pos_by_sym.get(sym)
                mv = getattr(pos, 'marketValue', None)
                market_val = abs(mv) if mv is not None else (abs(pos.position) * pos.avgCost if pos else 0)
                broker_weight = market_val / capital if (capital > 0 and market_val > 0) else 0
                if broker_weight > 0:
                    reconciled_shorts[sym] = broker_weight
                    if abs(broker_weight - 1.0 / core.NAMES_PER_SIDE) / (1.0 / core.NAMES_PER_SIDE) > 0.5:
                        import logging
                        logging.getLogger(__name__).warning(
                            f"New position {sym}: broker weight {broker_weight:.3f} differs from default {1.0 / core.NAMES_PER_SIDE:.3f}")
                else:
                    reconciled_shorts[sym] = 1.0 / core.NAMES_PER_SIDE
                discrepancies.append(f"  {sym}: IBKR=short({qty}), state=none → added (weight={reconciled_shorts[sym]:.3f})")
        else:
            if sym in state_longs:
                discrepancies.append(f"  {sym}: state=long, IBKR=flat → removed")
            elif sym in state_shorts:
                discrepancies.append(f"  {sym}: state=short, IBKR=flat → removed")

        # Check size mismatch for positions present in both broker and state.
        if qty != 0 and sym in pos_by_sym:
            pos = pos_by_sym[sym]
            # Fix 2: use current market value, not cost basis (avgCost), to avoid
            # spurious mismatches on positions deep in profit.
            mv = getattr(pos, 'marketValue', None)
            if mv and abs(mv) > 0:
                broker_weight = abs(mv) / capital
            else:
                import logging
                logging.getLogger(__name__).warning(
                    f"Reconcile {sym}: marketValue unavailable, falling back to avgCost")
                broker_weight = (abs(pos.position) * pos.avgCost) / capital
            if qty > 0:
                state_weight = state_longs.get(sym, 0)
            else:
                state_weight = state_shorts.get(sym, 0)
            if state_weight and abs(broker_weight - state_weight) / max(state_weight, 0.001) > 0.5:
                import logging
                logger = logging.getLogger(__name__)
                logger.warning(f"Reconcile size mismatch {sym}: state={state_weight:.3f} broker={broker_weight:.3f}")
                print(f"  WARNING: size mismatch {sym}: state={state_weight:.3f} broker={broker_weight:.3f} → updating state", flush=True)
                # Update state to broker-implied weight
                if sym in state_longs:
                    reconciled_longs[sym] = broker_weight
                elif sym in state_shorts:
                    reconciled_shorts[sym] = broker_weight

    if discrepancies:
        print(f"\n  POSITION RECONCILIATION ({len(discrepancies)} changes):", flush=True)
        for d in discrepancies[:15]:
            print(d, flush=True)
    else:
        print(f"  Positions reconciled: {len(reconciled_longs)}L / {len(reconciled_shorts)}S", flush=True)

    state["current_longs"] = reconciled_longs
    state["current_shorts"] = reconciled_shorts
    return state


def get_account_equity(ib) -> float:
    summary = ib.accountSummary(IBKR_ACCOUNT)
    for item in summary:
        if item.tag == "NetLiquidation" and item.currency == "USD":
            return float(item.value)
    msg = "FATAL: Could not fetch NetLiquidation from IBKR account summary — aborting to avoid silent $1M default sizing"
    send_telegram(f"<b>ML IBKR Premarket FATAL</b>\n{html.escape(msg)}")
    raise RuntimeError(msg)


def _cancel_gtc_stops(ib, sym: str) -> int:
    """Cancel all open GTC STP orders for sym. Returns count cancelled."""
    import logging
    logger = logging.getLogger(__name__)
    cancelled = 0
    for t in ib.openTrades():
        if (getattr(t.contract, "symbol", None) == sym
                and t.order.orderType.upper() == "STP"
                and t.order.tif in ("GTC", "DAY", "GTD")):
            logger.info(f"Cancelling existing stop for {sym}: orderId={t.order.orderId} tif={t.order.tif}")
            ib.cancelOrder(t.order)
            cancelled += 1
    if cancelled:
        print(f"      Cancelled {cancelled} GTC stop(s) for {sym}", flush=True)
    return cancelled


def execute_trades(ib, trades: list[dict]):
    import logging
    from ib_insync import Stock, LimitOrder, StopOrder
    logger = logging.getLogger(__name__)
    executed, failed, naked_positions = [], [], []

    # Verify gateway is alive before touching any orders.
    if not check_ibkr_connected(ib):
        msg = "IB Gateway disconnected before order placement — aborting session"
        logger.error(msg)
        print(f"  ERROR: {msg}", flush=True)
        send_telegram(f"🚨 <b>ML IBKR Premarket ABORTED</b>\n{html.escape(msg)}")
        raise RuntimeError(msg)

    # Snapshot current positions for stop management.
    current_qty: dict[str, int] = {}
    for pos in ib.positions():
        if pos.account == IBKR_ACCOUNT:
            sym_ = getattr(pos.contract, "symbol", None)
            if sym_:
                current_qty[sym_] = int(pos.position)

    for trade in trades:
        sym = trade["symbol"]
        action = trade["action"]
        shares = trade["shares"]
        close = trade["close"]

        # Bug 2 fix: guard against NaN close price before any price arithmetic.
        # ICE (and other illiquid symbols) can arrive with close=nan from scores,
        # which propagates into limit_price and then into the StopOrder aux price,
        # causing Error 320 from the gateway.
        if close is None or not math.isfinite(close):
            logger.error(f"{sym}: close price is {close} — skipping trade entirely")
            print(f"    SKIPPED {sym}: close price is nan/inf", flush=True)
            failed.append({**trade, "error": f"close price is {close}"})
            continue

        limit_price = round(close * (1 + ORDER_OFFSET_PCT), 2) if action == "BUY" \
                      else round(close * (1 - ORDER_OFFSET_PCT), 2)
        contract = Stock(sym, "SMART", "USD")

        try:
            ib.qualifyContracts(contract)
            is_new_entry = trade.get("weight_delta", 0) > 0

            # Cancel ALL existing GTC stops for this symbol before placing any
            # new stop — prevents duplicate GTC stops accumulating across rebalances.
            _cancel_gtc_stops(ib, sym)
            ib.sleep(0.5)

            # Compute total position size after this trade so the replacement
            # stop covers the full resulting position.
            # NOTE: total_after is intentionally pre-computed with `shares` here
            # only to build the stop order object. It will be CORRECTED to use
            # filled_qty after the fill poll (see below) — the stop is not
            # transmitted until after the bracket is confirmed.
            current_abs = abs(current_qty.get(sym, 0))
            if is_new_entry:
                # Scale-up or new entry: total position grows.
                total_after_requested = current_abs + shares
            else:
                # Reduction or exit: total position shrinks.
                total_after_requested = max(0, current_abs - shares)

            # Build a single GTC stop sized to the post-trade position.
            # Use total_after_requested as a placeholder; qty will be corrected
            # to filled_qty after the fill poll (stop is GTC so it persists, and
            # we will cancel+replace if fill was partial — handled below).
            stop = None
            stop_price = None
            if total_after_requested > 0:
                if trade["side"] == "long":
                    stop_price = round(limit_price * (1 - STOP_LOSS_PCT), 2)
                    stop = StopOrder("SELL", total_after_requested, stop_price, tif="GTC")
                elif trade["side"] == "short":
                    stop_price = round(limit_price * (1 + STOP_LOSS_PCT), 2)
                    stop = StopOrder("BUY", total_after_requested, stop_price, tif="GTC")

            order = LimitOrder(action, shares, limit_price, tif="DAY")
            order.account = IBKR_ACCOUNT
            order.outsideRth = True  # allow fills in premarket/afterhours
            # If we have a stop to bracket, hold transmission until stop is attached.
            order.transmit = stop is None

            entry_order = order  # keep reference for bracket-failure cancellation
            ib_trade = ib.placeOrder(contract, order)
            print(f"    {action} {shares} {sym} @ limit {limit_price:.2f} "
                  f"(orderId={ib_trade.order.orderId})", flush=True)

            if stop is not None:
                # Poll up to 60s (120×0.5s) before attaching bracket stop.
                for _i in range(120):  # 120 × 0.5s = 60s
                    ib.sleep(0.5)
                    if ib_trade.orderStatus.status in ('PreSubmitted', 'Submitted', 'Filled'):
                        break
                    if _i == 59:  # 30s elapsed
                        if ib_trade.orderStatus.status == 'PendingSubmit':
                            logger.warning(f"{sym}: order still PendingSubmit after 30s")
                            send_telegram(f"⚠️ {sym}: order stuck in PendingSubmit after 30s")
                entry_status = ib_trade.orderStatus.status
                if entry_status == 'PendingSubmit':
                    # Orders stuck in PendingSubmit after 60s means the gateway
                    # has disconnected mid-session (API port dropped while the
                    # socket still appears open).  Abort the entire run — further
                    # orders will also hang, burning the systemd timeout budget.
                    if not check_ibkr_connected(ib):
                        abort_msg = (
                            f"IB Gateway disconnected mid-session (order {sym} stuck "
                            f"PendingSubmit for 60s) — aborting to avoid systemd timeout"
                        )
                    else:
                        abort_msg = (
                            f"Order {sym} stuck PendingSubmit for 60s (gateway reports "
                            f"connected but order never acknowledged) — aborting session"
                        )
                    logger.error(abort_msg)
                    print(f"  ERROR: {abort_msg}", flush=True)
                    send_telegram(f"🚨 <b>ML IBKR Premarket ABORTED</b>\n{html.escape(abort_msg)}")
                    raise RuntimeError(abort_msg)
                if entry_status not in ('PreSubmitted', 'Submitted', 'Filled'):
                    logger.error(
                        f"Entry for {sym} unexpected state {entry_status} after 60s, skipping bracket")
                    continue

                stop.account = IBKR_ACCOUNT
                stop.parentId = ib_trade.order.orderId
                stop.transmit = True
                try:
                    ib.placeOrder(contract, stop)
                    print(f"      + STOP @ {stop_price:.2f} GTC {total_after_requested} shares "
                          f"(bracket parentId={ib_trade.order.orderId})", flush=True)
                except Exception as e:
                    logger.error(f"Stop placement failed for {sym}: {e}, cancelling entry")
                    ib.cancelOrder(entry_order)
                    raise

            # Wait up to 30s for fill confirmation before updating state.
            deadline = time.time() + 30
            while time.time() < deadline:
                ib.sleep(1)
                status = ib_trade.orderStatus.status
                if status in ('Filled', 'PartiallyFilled', 'Cancelled', 'Inactive'):
                    break

            filled_qty = int(ib_trade.orderStatus.filled or 0)
            if filled_qty > 0:
                # Fix 1: compute total_after from filled_qty, not requested shares.
                if is_new_entry:
                    total_after = current_abs + filled_qty
                else:
                    total_after = max(0, current_abs - filled_qty)

                # Fix 4: update current_qty so subsequent trades on the same symbol
                # use the correct post-fill baseline for total_after.
                if action == 'BUY':
                    current_qty[sym] = current_qty.get(sym, 0) + filled_qty
                else:
                    current_qty[sym] = current_qty.get(sym, 0) - filled_qty

                # Recompute stop_price from actual fill price, not close/limit_price.
                fill_price = ib_trade.orderStatus.avgFillPrice or close
                if fill_price is None or not math.isfinite(fill_price):
                    logger.warning(f"{sym}: skipping stop, fill_price is {fill_price}")
                    trade_copy = dict(trade)
                    trade_copy['shares'] = filled_qty
                    naked_positions.append(trade_copy)
                    send_telegram(f"⚠️ {sym}: fill price NaN — position has no stop, manual review needed")
                    print(f"      Filled {filled_qty}/{shares} shares (status={ib_trade.orderStatus.status}) — NaN fill price, NOT added to state", flush=True)
                    continue
                if trade["side"] == "long":
                    stop_price = round(fill_price * (1 - STOP_LOSS_PCT), 2)
                else:
                    stop_price = round(fill_price * (1 + STOP_LOSS_PCT), 2)

                # Cancel and replace the bracket stop if:
                #   (a) partial fill — stop was sized to total_after_requested, or
                #   (b) full fill but fill price differs from limit price by >0.1%
                #       (bracket stop was anchored to limit_price, not actual fill).
                if stop is not None and total_after > 0:
                    limit_based_stop = round(limit_price * (1 - STOP_LOSS_PCT), 2) \
                        if trade["side"] == "long" \
                        else round(limit_price * (1 + STOP_LOSS_PCT), 2)
                    price_drifted = (
                        abs(stop_price - limit_based_stop) / max(limit_based_stop, 0.01) > 0.001
                    )
                    needs_replace = (filled_qty < shares) or price_drifted
                    if needs_replace:
                        _cancel_gtc_stops(ib, sym)
                        ib.sleep(0.5)
                        if trade["side"] == "long":
                            corrected_stop = StopOrder("SELL", total_after, stop_price)
                        else:
                            corrected_stop = StopOrder("BUY", total_after, stop_price)
                        corrected_stop.tif = "GTC"
                        corrected_stop.account = IBKR_ACCOUNT
                        corrected_stop.outsideRth = True
                        corrected_stop.transmit = True
                        try:
                            corrected_trade = ib.placeOrder(contract, corrected_stop)
                            reason = "Partial fill" if filled_qty < shares else "Fill-price drift"
                            print(f"      {reason}: replaced stop → STOP @ {stop_price:.2f} "
                                  f"GTC {total_after} shares (was {total_after_requested})", flush=True)
                            # Poll submission status up to 3s.
                            for _ in range(6):
                                ib.sleep(0.5)
                                if corrected_trade.orderStatus.status in ('PreSubmitted', 'Submitted', 'Filled'):
                                    break
                            if corrected_trade.orderStatus.status not in ('PreSubmitted', 'Submitted', 'Filled'):
                                logger.error(f"Corrected stop for {sym} not submitted "
                                             f"(status={corrected_trade.orderStatus.status})")
                                send_telegram(f"⚠️ Corrected stop FAILED to submit for {html.escape(sym)}"
                                              f" — position may be unprotected")
                        except Exception as e:
                            logger.error(f"Corrected stop placement failed for {sym}: {e}")

                trade_copy = dict(trade)
                trade_copy['shares'] = filled_qty
                executed.append(trade_copy)
                print(f"      Filled {filled_qty}/{shares} shares (status={ib_trade.orderStatus.status})", flush=True)
            else:
                logger.warning(f"Order for {sym} not filled within timeout, skipping state update")
                print(f"      WARNING: {sym} order not filled within 30s (status={ib_trade.orderStatus.status}), skipping state update", flush=True)
                # Fix 1+2: entry unfilled but stop was already cancelled — re-place
                # a protective stop if the position still exists.
                _cancel_gtc_stops(ib, sym)  # clean up any lingering bracket child
                ib.sleep(0.5)
                if current_abs > 0:
                    side = "SELL" if trade["side"] == "long" else "BUY"
                    ticker = None
                    try:
                        ticker = ib.reqMktData(contract, '', False, False)
                        ib.sleep(1)
                        current_price = ticker.marketPrice() or close
                    except Exception:
                        current_price = close
                    finally:
                        if ticker is not None:
                            try:
                                ib.cancelMktData(contract)
                            except Exception:
                                pass
                    # Bug 2 fix: guard NaN/inf current_price before stop arithmetic.
                    if current_price is None or not math.isfinite(current_price):
                        logger.error(f"Entry unfilled for {sym}: current_price is {current_price}, cannot place protective stop")
                        print(f"      SKIPPED protective stop for {sym}: current_price is nan/inf", flush=True)
                        send_telegram(f"🚨 Entry unfilled for {html.escape(sym)} AND current_price=nan — cannot place protective stop, manual review needed")
                        continue
                    if trade["side"] == "long":
                        protective_stop_price = round(current_price * (1 - STOP_LOSS_PCT), 2)
                    else:
                        protective_stop_price = round(current_price * (1 + STOP_LOSS_PCT), 2)
                    replacement = StopOrder(side, current_abs, protective_stop_price, tif="GTC")
                    replacement.account = IBKR_ACCOUNT
                    replacement.transmit = True
                    try:
                        ib.placeOrder(contract, replacement)
                        logger.warning(f"Entry unfilled for {sym}, re-placed protective stop at {protective_stop_price}")
                        print(f"      Re-placed protective stop for {sym} @ {protective_stop_price:.2f} GTC {current_abs} shares", flush=True)
                        send_telegram(f"⚠️ Entry unfilled for {html.escape(sym)}, re-placed protective stop at {protective_stop_price}")
                    except Exception as e:
                        logger.error(f"Protective stop re-placement failed for {sym}: {e}")
                        send_telegram(f"🚨 Entry unfilled for {html.escape(sym)} AND stop re-placement failed: {html.escape(str(e))}")

        except Exception as e:
            print(f"    FAILED {action} {shares} {sym}: {e}", flush=True)
            failed.append({**trade, "error": str(e)})

    return executed, failed


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
def main():
    import ml_strategy_core as core

    dry_run = "--dry-run" in sys.argv
    print(f"=== ML PREMARKET IBKR {'(DRY RUN) ' if dry_run else ''}===\n", flush=True)

    scores_data = core.load_scores()
    score_date = scores_data["date"]
    print(f"Score date: {score_date}  Symbols: {len(scores_data['scores'])}", flush=True)

    state = core.load_state(STATE_PATH)

    # Connect IBKR for reconciliation + equity (skip in dry-run)
    ib = None
    if not dry_run:
        print("Connecting to IBKR...", flush=True)
        ib = connect_ibkr()

    try:
        if not dry_run:
            try:
                state["capital"] = get_account_equity(ib)
                print(f"  Account equity: ${state['capital']:,.0f}", flush=True)
                state = reconcile_positions(ib, state, state["capital"])

                # Startup broker reconciliation log — surface any state/broker drift.
                import logging as _logging
                _logger = _logging.getLogger(__name__)
                broker_positions = {p.contract.symbol: p for p in ib.positions()}
                state_syms = set(state.get('current_longs', {}).keys()) | set(state.get('current_shorts', {}).keys())
                broker_syms = set(broker_positions.keys())
                in_state_not_broker = state_syms - broker_syms
                in_broker_not_state = broker_syms - state_syms
                if in_state_not_broker:
                    _logger.warning(f"State/broker drift — in state not broker: {sorted(in_state_not_broker)}")
                if in_broker_not_state:
                    _logger.warning(f"State/broker drift — in broker not state: {sorted(in_broker_not_state)}")
            except Exception as e:
                print(f"ERROR: IBKR setup failed: {e}", flush=True)
                send_telegram(f"<b>ML IBKR Premarket ERROR</b> ({html.escape(str(score_date))})\n{html.escape(str(e))}")
                sys.exit(1)

        trades, state, plan_type = core.plan_trades(state, scores_data)

        if plan_type == "hold":
            if not dry_run:
                core.save_state(state, STATE_PATH)
            return

        print(f"\n{len(trades)} trades to execute (plan_type={plan_type})", flush=True)

        if dry_run:
            for t in trades:
                lp = round(t["close"] * (1 + ORDER_OFFSET_PCT), 2) if t["action"] == "BUY" \
                     else round(t["close"] * (1 - ORDER_OFFSET_PCT), 2)
                print(f"  [DRY RUN] {t['action']} {t['shares']} {t['symbol']} @ {lp:.2f} "
                      f"side={t['side']}", flush=True)
            return

        executed, failed = execute_trades(ib, trades)

        for t in executed:
            sym = t["symbol"]
            delta = t.get("weight_delta_unscaled", t["weight_delta"])
            side = t["side"]
            if side == "long":
                old_w = state["current_longs"].get(sym, 0.0)
                new_w = old_w + delta
                if new_w > 0.001:
                    state["current_longs"][sym] = new_w
                else:
                    state["current_longs"].pop(sym, None)
            else:
                old_w = state["current_shorts"].get(sym, 0.0)
                new_w = old_w + delta
                if new_w > 0.001:
                    state["current_shorts"][sym] = new_w
                else:
                    state["current_shorts"].pop(sym, None)

        core.save_state(state, STATE_PATH)

        n_long = len(state["current_longs"])
        n_short = len(state["current_shorts"])
        msg = (f"<b>ML IBKR Premarket</b> ({html.escape(str(score_date))})\n"
               f"Executed: {len(executed)} orders ({len(failed)} failed)\n"
               f"Portfolio: {n_long}L / {n_short}S\n"
               f"Capital: ${state['capital']:,.0f}")
        if failed:
            failed_syms = ", ".join(html.escape(t['symbol']) for t in failed[:5])
            msg += f"\nFailed: {failed_syms}"
        send_telegram(msg)

    finally:
        if ib is not None:
            ib.disconnect()


if __name__ == "__main__":
    main()
