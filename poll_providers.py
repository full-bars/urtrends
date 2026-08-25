#!/usr/bin/env python3
import sqlite3
import json
import requests
from datetime import datetime
from pathlib import Path
import time
import random
import os
import sys

BASE = Path.home() / "provider_tracking"
DB_PATH = BASE / "providers.db"
JWT_FILE = Path.home() / ".urnetwork" / "jwt"
LOG_FILE = BASE / "poll.log"
ENV_FILE = BASE / ".env"
FAIL_FILE = BASE / ".poll_fail_count"

def load_env():
    url = None
    if ENV_FILE.exists():
        for line in ENV_FILE.read_text().splitlines():
            if line.startswith("DISCORD_WEBHOOK_URL="):
                url = line.split("=", 1)[1].strip()
    return url

def send_discord(msg):
    url = load_env()
    if not url:
        return
    try:
        requests.post(url, json={"content": msg}, timeout=10)
    except:
        pass

def init_db():
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS provider_counts (
            timestamp TEXT NOT NULL,
            country_code TEXT NOT NULL,
            country_name TEXT NOT NULL,
            provider_count INTEGER NOT NULL,
            PRIMARY KEY (timestamp, country_code)
        )
    """)
    cursor.execute("""
        CREATE INDEX IF NOT EXISTS idx_country_timestamp
        ON provider_counts(country_code, timestamp)
    """)
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS ath_atl (
            type TEXT PRIMARY KEY,
            value INTEGER NOT NULL,
            timestamp TEXT NOT NULL
        )
    """)
    conn.commit()
    conn.close()

def poll_api():
    if not JWT_FILE.exists():
        log_message(f"JWT file not found: {JWT_FILE}")
        sys.exit(1)
    jwt_raw = JWT_FILE.read_text().strip()
    if not jwt_raw:
        log_message(f"JWT file is empty: {JWT_FILE}")
        sys.exit(1)
    jwt = jwt_raw
    headers = {"Authorization": f"Bearer {jwt}"}
    for attempt in range(3):
        response = None
        try:
            response = requests.get(
                "https://api.bringyour.com/network/provider-locations",
                headers=headers,
                timeout=30
            )
            response.raise_for_status()
            return response.json()
        except Exception as e:
            resp_detail = ""
            if response is not None:
                resp_detail = f", status={response.status_code}, body={response.text[:500]}"
            if attempt < 2:
                jitter = random.uniform(1, 2 ** attempt * 10)
                log_message(f"Poll attempt {attempt+1} failed: {e}{resp_detail}, retrying in {jitter:.0f}s")
                time.sleep(jitter)
            else:
                log_message(f"Poll attempt {attempt+1} failed: {e}{resp_detail}, giving up")
                raise

def store_data(data):
    timestamp = datetime.utcnow().strftime("%Y-%m-%d %H:%M:%S")
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()

    for location in data.get("locations", []):
        cursor.execute(
            "INSERT OR REPLACE INTO provider_counts VALUES (?, ?, ?, ?)",
            (
                timestamp,
                location["country_code"],
                location["name"],
                location["provider_count"]
            )
        )

    current_total = sum(loc["provider_count"] for loc in data.get("locations", []))
    update_ath_atl(cursor, current_total, timestamp)

    conn.commit()
    conn.close()

def update_ath_atl(cursor, current_total, timestamp):
    cursor.execute("SELECT value, timestamp FROM ath_atl WHERE type = 'ath'")
    ath_row = cursor.fetchone()
    if ath_row is None or current_total > ath_row[0]:
        prev = None if ath_row is None else ath_row[0]
        cursor.execute("INSERT OR REPLACE INTO ath_atl (type, value, timestamp) VALUES ('ath', ?, ?)", (current_total, timestamp))
        if ath_row is None:
            log_message(f"ATH initialized: {current_total} at {timestamp}")
        else:
            log_message(f"New ATH: {current_total} (was {ath_row[0]}) at {timestamp}")
            alerts.alert_milestone('ath', current_total, prev)

    cursor.execute("SELECT value, timestamp FROM ath_atl WHERE type = 'atl'")
    atl_row = cursor.fetchone()
    if atl_row is None or current_total < atl_row[0]:
        prev = None if atl_row is None else atl_row[0]
        cursor.execute("INSERT OR REPLACE INTO ath_atl (type, value, timestamp) VALUES ('atl', ?, ?)", (current_total, timestamp))
        if atl_row is None:
            log_message(f"ATL initialized: {current_total} at {timestamp}")
        else:
            log_message(f"New ATL: {current_total} (was {atl_row[0]}) at {timestamp}")
            alerts.alert_milestone('atl', current_total, prev)

def log_message(msg):
    with open(LOG_FILE, "a") as f:
        f.write(f"{datetime.now().isoformat()} — {msg}\n")

def track_failures(success):
    count = 0
    if FAIL_FILE.exists():
        count = int(FAIL_FILE.read_text().strip())
    if success:
        if count > 0:
            FAIL_FILE.write_text("0")
    else:
        count += 1
        FAIL_FILE.write_text(str(count))
        if count >= 3:
            send_discord(f"🔴 **Provider poller failed {count} times consecutively** — check poll.log")
    return count

import alerts

if __name__ == "__main__":
    try:
        init_db()
        data = poll_api()
        store_data(data)
        alerts.run_all()
        track_failures(True)
        log_message("Poll successful")
    except Exception as e:
        track_failures(False)
        log_message(f"ERROR: {e}")
        exit(1)
