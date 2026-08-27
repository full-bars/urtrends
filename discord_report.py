#!/usr/bin/env python3
import sqlite3
import requests
import argparse
import sys
import os
from datetime import datetime, timedelta
from pathlib import Path

WEBHOOK_URL = os.getenv("DISCORD_WEBHOOK_URL")
if not WEBHOOK_URL:
    print("Error: DISCORD_WEBHOOK_URL environment variable not set", file=sys.stderr)
    sys.exit(1)

DB_PATH = Path.home() / "urtrends" / "providers.db"

def get_db():
    conn = sqlite3.connect(str(DB_PATH))
    conn.row_factory = sqlite3.Row
    return conn

def send_discord(embeds):
    """Send embeds to Discord webhook."""
    payload = {"embeds": embeds}
    response = requests.post(WEBHOOK_URL, json=payload)
    if response.status_code != 204:
        print(f"Discord error: {response.status_code} {response.text}", file=sys.stderr)
    return response.status_code == 204

def get_movers(since_timestamp, window_name):
    """Get top gainers and losers since a specific timestamp."""
    conn = get_db()
    cursor = conn.cursor()
    
    latest = cursor.execute("SELECT MAX(timestamp) FROM provider_counts").fetchone()[0]
    
    # Gainers
    cursor.execute(f"""
        WITH current AS (
            SELECT country_code, country_name, provider_count
            FROM provider_counts WHERE timestamp = ?
        ),
        past AS (
            SELECT country_code, provider_count
            FROM provider_counts
            WHERE timestamp = (
                SELECT MAX(timestamp) FROM provider_counts WHERE timestamp <= ?
            )
        )
        SELECT c.country_name, c.country_code, c.provider_count,
               COALESCE(c.provider_count - p.provider_count, 0) as delta
        FROM current c
        LEFT JOIN past p ON c.country_code = p.country_code
        WHERE c.provider_count > 0
        ORDER BY delta DESC
        LIMIT 10
    """, (latest, since_timestamp))
    gainers = [dict(row) for row in cursor.fetchall()]
    
    # Losers
    cursor.execute(f"""
        WITH current AS (
            SELECT country_code, country_name, provider_count
            FROM provider_counts WHERE timestamp = ?
        ),
        past AS (
            SELECT country_code, provider_count
            FROM provider_counts
            WHERE timestamp = (
                SELECT MAX(timestamp) FROM provider_counts WHERE timestamp <= ?
            )
        )
        SELECT c.country_name, c.country_code, c.provider_count,
               COALESCE(c.provider_count - p.provider_count, 0) as delta
        FROM current c
        LEFT JOIN past p ON c.country_code = p.country_code
        WHERE c.provider_count > 0
        ORDER BY delta ASC
        LIMIT 10
    """, (latest, since_timestamp))
    losers = [dict(row) for row in cursor.fetchall()]
    
    conn.close()
    return gainers, losers

def get_network_range(since_timestamp):
    """Get high and low network totals for a time period."""
    conn = get_db()
    cursor = conn.cursor()

    latest = cursor.execute("SELECT MAX(timestamp) FROM provider_counts").fetchone()[0]

    cursor.execute("""
        WITH totals AS (
            SELECT SUM(provider_count) as total, timestamp
            FROM provider_counts
            WHERE timestamp >= ? AND timestamp <= ?
            GROUP BY timestamp
        )
        SELECT MAX(total) as high, MIN(total) as low
        FROM totals
    """, (since_timestamp, latest))

    result = cursor.fetchone()
    conn.close()
    return result['high'] or 0, result['low'] or 0

def get_anomalies(threshold=0.15):
    """Get countries with >threshold (ratio, e.g. 0.15 = 15%) change in the last hour."""
    conn = get_db()
    cursor = conn.cursor()

    latest = cursor.execute("SELECT MAX(timestamp) FROM provider_counts").fetchone()[0]
    hour_ago = (datetime.fromisoformat(latest) - timedelta(hours=1)).isoformat(sep=' ')

    cursor.execute(f"""
        WITH current AS (
            SELECT country_code, country_name, provider_count
            FROM provider_counts WHERE timestamp = ?
        ),
        past AS (
            SELECT p.country_code, p.provider_count
            FROM provider_counts p
            INNER JOIN (
                SELECT country_code, MAX(timestamp) as ts
                FROM provider_counts WHERE timestamp <= ?
                GROUP BY country_code
            ) pt ON p.country_code = pt.country_code AND p.timestamp = pt.ts
        )
        SELECT c.country_name, c.country_code, c.provider_count,
               COALESCE(c.provider_count - p.provider_count, 0) as delta,
               CASE WHEN p.provider_count > 0
                    THEN CAST(c.provider_count - p.provider_count AS FLOAT) / p.provider_count
                    ELSE 0 END as pct_change
        FROM current c
        LEFT JOIN past p ON c.country_code = p.country_code
        WHERE ABS(CAST(c.provider_count - p.provider_count AS FLOAT) / NULLIF(p.provider_count, 0)) > ?
    """, (latest, hour_ago, threshold))
    all_anomalies = [dict(row) for row in cursor.fetchall()]

    gains = [a for a in all_anomalies if a['pct_change'] > 0]
    losses = [a for a in all_anomalies if a['pct_change'] < 0]

    gains.sort(key=lambda x: abs(x['pct_change']), reverse=True)
    losses.sort(key=lambda x: abs(x['pct_change']), reverse=True)

    conn.close()
    return gains + losses

def report_hourly():
    """Send hourly report."""
    conn = get_db()
    cursor = conn.cursor()

    latest = cursor.execute("SELECT MAX(timestamp) FROM provider_counts").fetchone()[0]
    hour_ago = (datetime.fromisoformat(latest) - timedelta(hours=1)).isoformat(sep=' ')

    # Current total
    cursor.execute("SELECT SUM(provider_count) as total FROM provider_counts WHERE timestamp = ?", (latest,))
    current_total = cursor.fetchone()['total']

    # Hour-ago total
    cursor.execute("SELECT SUM(provider_count) as total FROM provider_counts WHERE timestamp <= ? ORDER BY timestamp DESC LIMIT 1", (hour_ago,))
    hour_ago_total = cursor.fetchone()['total'] or current_total
    hour_delta = current_total - hour_ago_total

    conn.close()

    hour_high, hour_low = get_network_range(hour_ago)
    gainers, losers = get_movers(hour_ago, "1h")
    anomalies = get_anomalies(threshold=0.15)

    embeds = [{
        "title": "⏰ Hourly Provider Update",
        "color": 3066993 if hour_delta >= 0 else 15158332,
        "fields": [
            {
                "name": "Total Network Providers",
                "value": f"{current_total:,} ({hour_delta:+,} 1h) | Range: {hour_low:,} - {hour_high:,}",
                "inline": False
            },
            {
                "name": "🔥 Top 5 Growing (1h)",
                "value": "\n".join([f"**{g['country_name']}** ({g['country_code'].upper()}): {g['provider_count']:,} (+{g['delta']})" for g in gainers[:5]]),
                "inline": False
            },
            {
                "name": "❄️ Top 5 Declining (1h)",
                "value": "\n".join([f"**{l['country_name']}** ({l['country_code'].upper()}): {l['provider_count']:,} ({l['delta']})" for l in losers[:5]]),
                "inline": False
            }
        ],
        "timestamp": latest
    }]
    
    if anomalies:
        anomaly_text = " • ".join([f"**{a['country_name']}** {a['delta']:+d} ({a['pct_change']*100:+.1f}%)" for a in anomalies[:5]])
        embeds[0]["fields"].append({
            "name": "🚨 Anomalies Detected (>15% change)",
            "value": f"⚠️ Anomalies: {anomaly_text}",
            "inline": False
        })
    
    embeds[0]["footer"] = {"text": f"Snapshot at {latest} UTC"}
    
    return send_discord(embeds)

def report_daily():
    """Send daily summary."""
    conn = get_db()
    cursor = conn.cursor()

    latest = cursor.execute("SELECT MAX(timestamp) FROM provider_counts").fetchone()[0]
    day_ago = (datetime.fromisoformat(latest) - timedelta(days=1)).isoformat(sep=' ')

    # Totals
    cursor.execute("SELECT SUM(provider_count) as total FROM provider_counts WHERE timestamp = ?", (latest,))
    current_total = cursor.fetchone()['total']

    cursor.execute("SELECT SUM(provider_count) as total FROM provider_counts WHERE timestamp <= ? ORDER BY timestamp DESC LIMIT 1", (day_ago,))
    day_ago_total = cursor.fetchone()['total'] or current_total
    day_delta = current_total - day_ago_total

    # Top 10
    cursor.execute("SELECT country_name, country_code, provider_count FROM provider_counts WHERE timestamp = ? ORDER BY provider_count DESC LIMIT 10", (latest,))
    top_10 = [dict(row) for row in cursor.fetchall()]

    conn.close()

    day_high, day_low = get_network_range(day_ago)
    gainers, losers = get_movers(day_ago, "24h")

    top_10_str = "\n".join([f"{i}. **{c['country_name']}** ({c['country_code'].upper()}): {c['provider_count']:,}" for i, c in enumerate(top_10, 1)])
    gainers_str = "\n".join([f"**{g['country_name']}** (+{g['delta']})" for g in gainers[:5]])
    losers_str = "\n".join([f"**{l['country_name']}** ({l['delta']})" for l in losers[:5]])

    embeds = [{
        "title": "📊 Daily Summary",
        "color": 3066993 if day_delta >= 0 else 15158332,
        "fields": [
            {
                "name": "Total Network",
                "value": f"{current_total:,} ({day_delta:+,} 24h) | Range: {day_low:,} - {day_high:,}",
                "inline": False
            },
            {
                "name": "🏆 Top 10 Countries",
                "value": top_10_str,
                "inline": False
            },
            {
                "name": "📈 Top 5 Gainers",
                "value": gainers_str or "No change",
                "inline": True
            },
            {
                "name": "📉 Top 5 Losers",
                "value": losers_str or "No change",
                "inline": True
            }
        ],
        "timestamp": latest,
        "footer": {"text": f"Daily update for {latest[:10]}"}
    }]
    
    return send_discord(embeds)

def report_weekly():
    """Send weekly summary."""
    conn = get_db()
    cursor = conn.cursor()

    latest = cursor.execute("SELECT MAX(timestamp) FROM provider_counts").fetchone()[0]
    week_ago = (datetime.fromisoformat(latest) - timedelta(days=7)).isoformat(sep=' ')

    # Totals
    cursor.execute("SELECT SUM(provider_count) as total FROM provider_counts WHERE timestamp = ?", (latest,))
    current_total = cursor.fetchone()['total']

    cursor.execute("SELECT SUM(provider_count) as total FROM provider_counts WHERE timestamp <= ? ORDER BY timestamp DESC LIMIT 1", (week_ago,))
    week_ago_total = cursor.fetchone()['total'] or current_total
    week_delta = current_total - week_ago_total

    # Top 10
    cursor.execute("SELECT country_name, country_code, provider_count FROM provider_counts WHERE timestamp = ? ORDER BY provider_count DESC LIMIT 10", (latest,))
    top_10 = [dict(row) for row in cursor.fetchall()]

    conn.close()

    week_high, week_low = get_network_range(week_ago)
    gainers, losers = get_movers(week_ago, "7d")

    top_10_str = "\n".join([f"{i}. **{c['country_name']}** ({c['country_code'].upper()}): {c['provider_count']:,}" for i, c in enumerate(top_10, 1)])
    gainers_str = "\n".join([f"**{g['country_name']}** (+{g['delta']})" for g in gainers[:5]])
    losers_str = "\n".join([f"**{l['country_name']}** ({l['delta']})" for l in losers[:5]])

    embeds = [{
        "title": "📈 Weekly Summary",
        "color": 3066993 if week_delta >= 0 else 15158332,
        "fields": [
            {
                "name": "Total Network",
                "value": f"{current_total:,} ({week_delta:+,} 7d) | Range: {week_low:,} - {week_high:,}",
                "inline": False
            },
            {
                "name": "🏆 Top 10 Countries",
                "value": top_10_str,
                "inline": False
            },
            {
                "name": "📈 Top 5 Gainers (7d)",
                "value": gainers_str or "No change",
                "inline": True
            },
            {
                "name": "📉 Top 5 Losers (7d)",
                "value": losers_str or "No change",
                "inline": True
            }
        ],
        "timestamp": latest,
        "footer": {"text": f"Weekly update through {latest[:10]}"}
    }]
    
    return send_discord(embeds)

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--type", choices=["hourly", "daily", "weekly"], required=True)
    args = parser.parse_args()
    
    if args.type == "hourly":
        success = report_hourly()
    elif args.type == "daily":
        success = report_daily()
    elif args.type == "weekly":
        success = report_weekly()
    
    sys.exit(0 if success else 1)
