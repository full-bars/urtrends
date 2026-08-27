# URTrends

Real-time monitoring and analytics dashboard for URnetwork provider metrics across global regions.

## Features

- **Real-time Network Monitoring** — Live provider count tracking with 15-minute snapshots
- **Anomaly Detection** — Automatic detection of >15% provider count changes (configurable threshold)
- **Regional Analysis** — 6-region aggregation (North America, Europe, Asia-Pacific, Middle East, South America, Africa) with 24-hour deltas
- **Growth Analytics** — Daily growth rate projections with 30-day trends and volatility indicators
- **At-Risk Tracking** — Identifies countries losing providers and near-zero capacity regions
- **Multi-Country Comparison** — Dynamic side-by-side analysis of up to 10 countries simultaneously
- **Moving Averages** — 24-hour rolling average overlaid on network total trends
- **Historical Data** — Up to 30 days of historical snapshots for trend analysis
- **Smart Ticker** — Auto-fallback from 1H to 2H to 6H movers when the network is quiet
- **Live Poll Indicator** — Color-coded freshness dot with automatic stale-data alerts

## Tech Stack

- **Backend** — Rust Actix-web with SQLite (recommended) or Python Flask (alternative)
- **Frontend** — Vanilla JavaScript with Chart.js for visualizations
- **Data Collection** — Every 15 minutes via cron, polling the URnetwork API
- **Deployment** — systemd service on Linux
- **Reverse Proxy** — Caddy for HTTPS and multi-domain routing
- **CI/CD** — GitHub Actions builds and publishes Rust binaries on `v*` tag push

### Releases

Pre-built Rust binaries are published on GitHub Releases when a version tag is pushed. See **Installation** below for download instructions.

## Installation

### Quick Start (Rust, Recommended)

1. Clone the repository:
```bash
git clone https://github.com/full-bars/urtrends.git
cd urtrends
```

2. Create database directory:
```bash
mkdir -p ~/urtrends
```

3. Set up 15-minute data collection:
```bash
crontab -e
# Add line:
*/15 * * * * /path/to/poll_providers.py >> /var/log/urtrends.log 2>&1
```

4. Download the pre-built binary:
```bash
LATEST_TAG=$(curl -s https://api.github.com/repos/full-bars/urtrends/releases/latest | grep tag_name | cut -d '"' -f 4)
wget -O ~/urtrends/urtrends https://github.com/full-bars/urtrends/releases/download/${LATEST_TAG}/urtrends
chmod +x ~/urtrends/urtrends
```

5. Install and start the service:
```bash
sudo cp urtrends.service /etc/systemd/system/
sudo systemctl enable urtrends
sudo systemctl start urtrends
```

Dashboard available at `http://localhost:5001`

#### Build from Source (Alternative)

If you prefer to compile yourself:

```bash
cd backend-rs
cargo build --release
cp target/release/urtrends ~/urtrends/
# Then install the systemd service as above
```

### Alternative: Python Flask Backend

If you don't have Rust available or want a quicker setup for development:

```bash
# Same clone + directory + cron steps as above, then:
pip install flask

sudo cp urtrends-py.service /etc/systemd/system/
sudo systemctl enable urtrends-py
sudo systemctl start urtrends-py
```

Dashboard available at `http://localhost:5000`

### Running Both Simultaneously

Both backends share the same SQLite database and can run on different ports. Use a reverse proxy to route traffic:

```
# Caddy example
providers.yourdomain.com {
    reverse_proxy localhost:5000
}

providers-rs.yourdomain.com {
    reverse_proxy localhost:5001
}
```

## Usage

### Web Dashboard

Access the interactive dashboard at `http://<server>:5001` (Rust) or `:5000` (Python) to view:
- **Network Summary** — Total provider count, hourly/daily deltas
- **Top 10 Countries** — Bar chart of highest-capacity regions
- **Regional Breakdown** — Horizontal bar chart showing provider totals by region with color-coded deltas
- **Network Trend** — 30-day line chart with 24h moving average overlay and time-stamped x-axis
- **Distribution** — Donut chart showing top 10 countries + "Others" concentration
- **Anomalies** — Real-time scrolling alert banner for significant changes (>15% by default)
- **Gainers/Losers** — Top 50 movers with 15m-7d time deltas, volatility levels, and churn rates
- **At-Risk Countries** — Tracking disappeared countries (0 providers) and near-zero capacity regions (1–5 providers, declining)

### Customization

**Anomaly Threshold** — Adjust detection sensitivity in the dashboard:
- Input field next to anomaly banner (default: 15%)
- Value persisted to browser localStorage
- Automatically re-fetches and updates alerts

**Multi-Country Comparison** — Compare up to 10 countries side-by-side:
- Add/remove countries dynamically
- Supports country name search with common aliases (Netherlands/Holland, UK/England, South Korea/Korea, etc.)
- Displays provider count trends with time-stamped x-axis
- Defaults to top 5 countries on load

**Movers Time Windows** — View detailed deltas across multiple time intervals:
- 15 minutes to 7 days granularity
- Shows volatility level (high/medium/low) with churn rate (providers/hour)
- Top 50 gainers and losers for each metric

## API Endpoints

### `/api/summary`
Current network state with top 10 countries.
```json
{
  "timestamp": "2026-05-24T22:15:00",
  "total": 65459,
  "hour_delta": 120,
  "day_delta": -2000,
  "top_10": [...]
}
```

### `/api/network_total`
Hourly totals with 24h moving average (past 168 hours).
```json
[
  {
    "timestamp": "2026-05-24T21:00:00",
    "total": 65340,
    "ma": 64800
  }
]
```

### `/api/movers`
Gainers and losers by time window (1h, 24h, 7d).

### `/api/regions`
Regional aggregation with 24h deltas.
```json
[
  {
    "region": "North America",
    "total": 22000,
    "delta_24h": -500
  }
]
```

### `/api/at-risk`
Countries that disappeared (0 providers) or near-zero (1–5 providers).
```json
{
  "disappeared": [...],
  "near_zero": [...]
}
```

### `/api/anomalies`
Significant movers above threshold.
```
?threshold=15  (percentage, optional)
```

## Data Collection

The `poll_providers.py` script runs every 15 minutes and:
1. Authenticates with URnetwork API using JWT from `~/.urnetwork/jwt`
2. Fetches provider locations via `/api/network/provider-locations`
3. Aggregates counts by country_code and country_name
4. Inserts snapshot with current timestamp into database

Run manually:
```bash
python3 poll_providers.py
```

## Database Schema

```sql
CREATE TABLE provider_counts (
  id INTEGER PRIMARY KEY,
  timestamp TEXT NOT NULL,
  country_code TEXT NOT NULL,
  country_name TEXT NOT NULL,
  provider_count INTEGER NOT NULL,
  UNIQUE(timestamp, country_code)
);
```

## Architecture

### Data Flow
1. **Collection** — `poll_providers.py` → URnetwork API
2. **Storage** — SQLite database with 15-minute snapshots
3. **API** — Backend REST endpoints aggregate and compute deltas
4. **Frontend** — Chart.js visualizations with client-side interactivity

### Key Calculations
- **Moving Average** — 96-point rolling average of network totals (true 24h at 15-min spacing)
- **Anomalies** — (current - past) / past > threshold, absolute value
- **Regional Totals** — SUM(provider_count) grouped by REGIONS mapping
- **Growth Rate** — (current_total - 30d_ago) / 30d_ago * 100
- **Volatility** — Std dev of 24h deltas

## Configuration

The Rust backend auto-detects the database location:
- Default: `~/urtrends/providers.db`
- Override: Set `DATABASE_URL` environment variable

```bash
DATABASE_URL="sqlite:///var/urtrends/providers.db" ./target/release/urtrends
```

Server binds to `0.0.0.0:5001` by default.

## Deployment

```bash
# Deploy Rust backend
scp backend-rs/target/release/urtrends user@server:~/urtrends/
scp urtrends.service user@server:/tmp/
ssh user@server 'sudo mv /tmp/urtrends.service /etc/systemd/system/ && sudo systemctl daemon-reload && sudo systemctl enable urtrends && sudo systemctl restart urtrends'

# Deploy Python backend (alternative)
scp dashboard/app.py user@server:~/urtrends/dashboard/
scp urtrends-py.service user@server:/tmp/
ssh user@server 'sudo mv /tmp/urtrends-py.service /etc/systemd/system/ && sudo systemctl daemon-reload && sudo systemctl enable urtrends-py && sudo systemctl restart urtrends-py'
```

## Development

Run locally:
```bash
cd dashboard
python3 app.py
```

Server runs on `http://localhost:5000` with auto-reload disabled (edit `app.run()` to enable debug mode).

## License

Open source
