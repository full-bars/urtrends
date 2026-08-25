# Provider Tracking Dashboard

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

- **Backend** — Python Flask with SQLite **OR** Rust with Actix-web (feature-identical, your choice)
- **Frontend** — Vanilla JavaScript with Chart.js for visualizations
- **Data Collection** — Every 15 minutes via cron, polling the URnetwork API
- **Deployment** — systemd service on Linux
- **Reverse Proxy** — Caddy for HTTPS and multi-domain routing
- **CI/CD** — GitHub Actions builds and publishes Rust binaries on `v*` tag push

## Choosing Your Backend

Both backends are fully functional and produce identical results. Choose based on your preference:

| Feature | Python Flask | Rust Actix-web |
|---------|---------|---------|
| **Memory Usage** | ~50-100 MB | ~4-5 MB |
| **Startup Time** | ~1-2s | <100ms |
| **Setup** | Python 3.9+ | Rust 1.70+ (or [pre-built binary](https://github.com/full-bars/urtrends/releases)) |
| **Dependencies** | Minimal (Flask, requests) | Zero runtime dependencies |
| **Performance** | Good | Excellent |
| **Recommended For** | Quick setup, development | Production, resource-constrained environments |

Both share the same database and can run simultaneously on different ports.
- **Python**: `http://localhost:5000`
- **Rust**: `http://localhost:5001`

### Releases

Pre-built Rust binaries are published on GitHub Releases when a version tag is pushed. See **Installation** below for download instructions.

## Installation

### Common Setup (Both Backends)

1. Clone the repository:
```bash
git clone https://github.com/full-bars/urtrends.git
cd urtrends
```

2. Create database directory:
```bash
mkdir -p ~/provider_tracking
```

3. Set up 15-minute data collection:
```bash
# Add to crontab
crontab -e
# Add line:
*/15 * * * * /path/to/poll_providers.py >> /var/log/provider-tracking.log 2>&1
```

### Option 1: Python Flask Backend

**Prerequisites**: Python 3.9+, pip

```bash
# Install dependencies
pip install flask

# Configure and start
sudo cp provider-dashboard.service /etc/systemd/system/
sudo systemctl enable provider-dashboard
sudo systemctl start provider-dashboard
```

Dashboard available at `http://localhost:5000`

### Option 2: Rust Actix-web Backend

**Prerequisites**: Rust 1.70+ (for building from source)

**Option 2a: Use pre-built binary**
```bash
# Download binary from latest release
LATEST_TAG=$(curl -s https://api.github.com/repos/full-bars/urtrends/releases/latest | grep tag_name | cut -d '"' -f 4)
wget -O provider_tracker https://github.com/full-bars/urtrends/releases/download/${LATEST_TAG}/provider_tracker
chmod +x provider_tracker
sudo cp provider_tracker /home/user/provider_tracking/

# Configure and start
sudo cp provider-dashboard-rs.service /etc/systemd/system/
sudo systemctl enable provider-dashboard-rs
sudo systemctl start provider-dashboard-rs
```

**Option 2b: Build from source**
```bash
cd backend-rs
cargo build --release
cp target/release/provider_tracker /home/user/provider_tracking/

# Configure and start (as above)
```

Dashboard available at `http://localhost:5001`

### Using Both Simultaneously

You can run both backends on different ports (5000 and 5001) reading from the same database. Useful for testing or gradual migration. Use Caddy or your reverse proxy to route different domains to each:

```
providers.yoursite.com {
    reverse_proxy localhost:5000
}

providers-rs.yoursite.com {
    reverse_proxy localhost:5001
}
```

## Usage

### Web Dashboard

Access the interactive dashboard at `http://<server>:5000` to view:
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

The `poll_providers.sh` script runs hourly and:
1. Authenticates with URnetwork API using JWT from `~/.urnetwork/jwt`
2. Fetches provider locations via `/api/network/provider-locations`
3. Aggregates counts by country_code and country_name
4. Inserts snapshot with current timestamp into database

Run manually:
```bash
./poll_providers.sh
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
1. **Collection** — `poll_providers.sh` → URnetwork API
2. **Storage** — SQLite database with hourly snapshots
3. **API** — Flask REST endpoints aggregate and compute deltas
4. **Frontend** — Chart.js visualizations with client-side interactivity

### Key Calculations
- **Moving Average** — 24-point rolling average of network totals
- **Anomalies** — (current - past) / past > threshold, absolute value
- **Regional Totals** — SUM(provider_count) grouped by REGIONS mapping
- **Growth Rate** — (current_total - 30d_ago) / 30d_ago * 100
- **Volatility** — Std dev of 24h deltas

## Deployment

Deploy to LA1 server:
```bash
scp dashboard/app.py user@100.70.15.86:/home/user/provider_tracking/dashboard/app.py
ssh user@100.70.15.86 'sudo systemctl restart provider-dashboard'
```

## Development

Run locally:
```bash
cd dashboard
python3 app.py
```

Server runs on `http://localhost:5000` with auto-reload disabled (edit `app.run()` to enable debug mode).

## Building the Rust Backend from Source

If you prefer to build the Rust backend yourself:

```bash
cd backend-rs
cargo build --release
```

Binary output: `target/release/provider_tracker` (~10 MB, completely self-contained)

### Configuration

The Rust backend auto-detects the database location:
- Default: `~/provider_tracking/providers.db`
- Override: Set `DATABASE_URL` environment variable

```bash
# Start with custom database path
DATABASE_URL="sqlite:///var/provider_tracking/providers.db" ./target/release/provider_tracker
```

Server binds to `0.0.0.0:5001` by default.

### Remote Deployment

Deploy both backends to a remote server and route traffic via reverse proxy:

```bash
# Deploy Python backend
scp dashboard/app.py user@server:/home/user/provider_tracking/
scp provider-dashboard.service user@server:/tmp/
ssh user@server 'sudo mv /tmp/provider-dashboard.service /etc/systemd/system/ && sudo systemctl daemon-reload && sudo systemctl enable provider-dashboard && sudo systemctl restart provider-dashboard'

# Deploy Rust backend
scp backend-rs/target/release/provider_tracker user@server:/home/user/provider_tracking/
scp provider-dashboard-rs.service user@server:/tmp/
ssh user@server 'sudo mv /tmp/provider-dashboard-rs.service /etc/systemd/system/ && chmod +x /home/user/provider_tracking/provider_tracker && sudo systemctl daemon-reload && sudo systemctl enable provider-dashboard-rs && sudo systemctl restart provider-dashboard-rs'
```

Then configure your reverse proxy (Caddy, nginx, etc.) to route traffic:

```
# Caddy example
providers.yourdomain.com {
    reverse_proxy localhost:5000
}

providers-rs.yourdomain.com {
    reverse_proxy localhost:5001
}
```

Both backends read from the same SQLite database and can operate independently or in parallel.

## License

Open source
