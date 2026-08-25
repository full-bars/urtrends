#!/usr/bin/env python3
"""Network trend alerting for urtrends.

Discord alerts:
  - All-time high / all-time low milestones (fired from the single place
    that detects a new extreme, so it is reboot-safe and never duplicates)
  - Tiered network-delta alerts (5/10/15/20/25/50%+ change over 60 min)
    edge-triggered + recovery-aware + per-tier cooldown, tracked in
    alert_state.json
  - Staleness check (poller died / server down)

Milestone alerts are triggered by poll_providers.update_ath_atl() calling
alert_milestone() at the moment a new extreme is written to the DB. This
avoids any separate state file for milestones, so reboots cannot cause
duplicate notifications.
"""
import json
import os
import sqlite3
import requests
from datetime import datetime, timezone
from pathlib import Path

BASE = Path.home() / "provider_tracking"
DB_PATH = BASE / "providers.db"
ENV_FILE = BASE / ".env"
STATE_FILE = BASE / "alert_state.json"

# Webhook is loaded from DISCORD_WEBHOOK_URL in .env (or /home/<user>/provider_tracking/.discord_webhook).
# Never hardcode secrets. Defaults to None; alerts are skipped if unset.
DEFAULT_WEBHOOK = None

# Tiers are percentage deltas (absolute value) over the 60-min window.
# 2% intentionally omitted: regular network churn should not alert.
TIERS = [5, 10, 15, 20, 25, 50]
# Per-tier minimum cooldown in minutes. Serious moves can re-alert sooner.
TIER_COOLDOWN_MIN = {5: 360, 10: 180, 15: 120, 20: 90, 25: 60, 50: 30}
# Absolute floor: ignore moves smaller than this many providers regardless of %.
ABS_FLOOR = 15
# Window for delta comparison (minutes). Poll is every 15m, so 60m = 4 polls back.
DELTA_WINDOW_MIN = 60
# Staleness threshold (minutes since last successful poll).
STALE_MIN = 60


def webhook_url():
    if ENV_FILE.exists():
        for line in ENV_FILE.read_text().splitlines():
            if line.startswith("DISCORD_WEBHOOK_URL="):
                v = line.split("=", 1)[1].strip()
                if v:
                    return v
    webhook_file = BASE / ".discord_webhook"
    if webhook_file.exists():
        v = webhook_file.read_text().strip()
        if v:
            return v
    return DEFAULT_WEBHOOK


def send(embed=None, content=None):
    if os.environ.get("ALERTS_DRYRUN"):
        print(f"[DRYRUN] would send: embed={bool(embed)} content={bool(content)}")
        return
    url = webhook_url()
    if not url:
        return
    payload = {}
    if embed:
        payload["embeds"] = [embed]
    if content:
        payload["content"] = content
    try:
        requests.post(url, json=payload, timeout=10)
    except Exception:
        pass


def _now_ts():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def load_state():
    if STATE_FILE.exists():
        try:
            return json.loads(STATE_FILE.read_text())
        except Exception:
            pass
    return {}


def save_state(s):
    STATE_FILE.write_text(json.dumps(s, indent=2))


def _connect():
    return sqlite3.connect(DB_PATH)


def alert_milestone(metric_type, new_value, old_value):
    """Called by poll_providers.update_ath_atl() the moment a new extreme is
    written. old_value may be None on first initialization."""
    if old_value is None:
        return  # don't alert on first seed
    if metric_type == "ath":
        title = "🚀 New All-Time High"
        color = 5763719  # green
        desc = f"Network reached **{new_value:,}** providers (was {old_value:,})."
    else:
        title = "📉 New All-Time Low"
        color = 15158332  # red
        desc = f"Network dropped to **{new_value:,}** providers (was {old_value:,})."
    send(embed={
        "title": title,
        "description": desc,
        "color": color,
        "timestamp": _now_ts(),
    })


def current_total_and_history():
    """Return (current_total, total_60min_ago or None, last_ts, ts_60min_ago)."""
    conn = _connect()
    cur = conn.cursor()
    cur.execute(
        "SELECT timestamp, SUM(provider_count) FROM provider_counts "
        "WHERE timestamp = (SELECT MAX(timestamp) FROM provider_counts) "
        "GROUP BY timestamp"
    )
    row = cur.fetchone()
    if not row:
        conn.close()
        return None, None, None, None
    last_ts, current = row

    cur.execute(
        "SELECT timestamp, SUM(provider_count) FROM provider_counts "
        "WHERE timestamp <= datetime('now', '-%d minutes') "
        "GROUP BY timestamp ORDER BY timestamp DESC LIMIT 1" % DELTA_WINDOW_MIN
    )
    old = cur.fetchone()
    conn.close()
    if old:
        return current, old[1], last_ts, old[0]
    return current, None, last_ts, None


def check_delta(state):
    current, old, last_ts, old_ts = current_total_and_history()
    if current is None or old is None:
        return state

    delta = current - old
    pct = (abs(delta) / old * 100.0) if old else 0.0
    direction = "up" if delta > 0 else "down"

    if abs(delta) < ABS_FLOOR:
        state["trend"] = "flat"
        state["last_alerted_tier"] = 0
        save_state(state)
        return state

    tier = 0
    for t in TIERS:
        if pct >= t:
            tier = t
    if tier == 0:
        state["trend"] = direction
        save_state(state)
        return state

    now = datetime.now(timezone.utc)
    last_alert_ts = state.get("last_delta_alert_ts")
    last_tier = state.get("last_alerted_tier", 0)
    trend = state.get("trend", "flat")

    cooldown = TIER_COOLDOWN_MIN.get(tier, 60)
    cooled = (last_alert_ts is None) or (
        (now - datetime.fromisoformat(last_alert_ts.replace("Z", "+00:00"))).total_seconds()
        >= cooldown * 60
    )

    worsening = tier > last_tier
    recovery = (trend == "down" and direction == "up")

    if (worsening or recovery) and (cooled or recovery):
        if worsening:
            title = f"📊 Network {'surge' if direction == 'up' else 'drop'}: {pct:.0f}% in {DELTA_WINDOW_MIN}m"
            color = 16776960 if direction == "up" else 15158332
            desc = (f"**{old:,}** → **{current:,}** ({delta:+,}) over the last "
                    f"{DELTA_WINDOW_MIN} minutes ({pct:.1f}% {direction}).")
            send(embed={"title": title, "description": desc, "color": color,
                        "timestamp": _now_ts()})
            state["last_alerted_tier"] = tier
            state["last_delta_alert_ts"] = now.strftime("%Y-%m-%dT%H:%M:%SZ")
        elif recovery:
            send(embed={
                "title": "✅ Network recovering",
                "description": f"Rebounded to **{current:,}** providers after a drop (was {old:,}).",
                "color": 5763719,
                "timestamp": _now_ts(),
            })
            state["last_delta_alert_ts"] = now.strftime("%Y-%m-%dT%H:%M:%SZ")
            state["last_alerted_tier"] = 0

    state["trend"] = direction
    save_state(state)
    return state


def check_staleness(state):
    conn = _connect()
    cur = conn.cursor()
    cur.execute("SELECT MAX(timestamp) FROM provider_counts")
    last_ts = cur.fetchone()[0]
    conn.close()
    if not last_ts:
        return state
    try:
        last_dt = datetime.strptime(last_ts, "%Y-%m-%d %H:%M:%S").replace(tzinfo=timezone.utc)
    except Exception:
        return state
    diff_min = int((datetime.now(timezone.utc) - last_dt).total_seconds() / 60)
    if diff_min > STALE_MIN:
        last_stale = state.get("last_stale_ts")
        if last_stale is None or (
            datetime.now(timezone.utc)
            - datetime.fromisoformat(last_stale.replace("Z", "+00:00"))
        ).total_seconds() >= STALE_MIN * 60:
            ago = f"{diff_min // 60}h {diff_min % 60}m" if diff_min >= 60 else f"{diff_min}m"
            send(embed={
                "title": "🚨 Provider Tracker STALE",
                "description": f"Last poll: **{last_ts}** ({ago} ago). Server may be down or the poller stopped. No data collected.",
                "color": 15158332,
                "timestamp": _now_ts(),
            })
            state["last_stale_ts"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    else:
        state.pop("last_stale_ts", None)
    return state


def run_all():
    state = load_state()
    state = check_delta(state)
    state = check_staleness(state)
    save_state(state)


if __name__ == "__main__":
    run_all()
