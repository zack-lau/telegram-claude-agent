# trading-crypto Plan 1: Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Scaffold the `trading-crypto` project on SGDGX01, implement the data fetching pipeline (ccxt → Parquet), data validation, on-chain data stubs, and triple-barrier labeling — all test-covered and committed.

**Architecture:** Custom Python project managed by uv. Data layer fetches 4h OHLCV for BTC/USDT and ETH/USDT from Binance via ccxt, stores snapshots as Parquet. On-chain module fetches free daily metrics and enforces publication lag before broadcasting to 4h candles. Triple-barrier labeling generates volatility-scaled 3-class labels with embargo.

**Tech Stack:** Python 3.12, uv 0.11.2, ccxt, pandas, pyarrow, requests, pytest. All commands run on SGDGX01 via SSH unless noted. uv is at `/home/agents/.local/bin/uv`.

**This plan covers:** Tasks 1–5 (Foundation). Plans 2–4 cover features+models, backtesting, and FreqAI live execution.

---

## File Map

| File | Responsibility |
|---|---|
| `pyproject.toml` | Project metadata, dependencies (Phase 1 only — torch deferred to Plan 2) |
| `data/__init__.py` | Package marker |
| `data/exceptions.py` | `DataValidationError` |
| `data/fetcher.py` | ccxt OHLCV fetch, Parquet save/load |
| `data/validate.py` | OHLC invariants, gap detection, timestamp checks |
| `data/onchain.py` | Free on-chain APIs + publish_lag + daily→4h broadcast + stubs |
| `labels/__init__.py` | Package marker |
| `labels/triple_barrier.py` | Vol-scaled triple-barrier labeling + embargo |
| `tests/__init__.py` | Package marker |
| `tests/test_fetcher.py` | Fetcher unit tests (mocked ccxt) |
| `tests/test_validate.py` | Validation unit tests (synthetic DataFrames) |
| `tests/test_onchain.py` | On-chain unit tests (mocked requests) |
| `tests/test_labels.py` | Triple-barrier unit tests (synthetic price series) |
| `configs/data_config.yaml` | Data source config (assets, timeframe, publish lags) |
| `README.md` | Setup + usage |

---

## Task 1: Project Scaffold

**Files:**
- Create: `/home/agents/trading-crypto/pyproject.toml`
- Create: `/home/agents/trading-crypto/README.md`
- Create: all `__init__.py` and directory placeholders
- Create: `/home/agents/trading-crypto/configs/data_config.yaml`

- [ ] **Step 1.1: Create project directory and initialize git**

```bash
ssh sgdgx01 "
mkdir -p /home/agents/trading-crypto
cd /home/agents/trading-crypto
git init
git checkout -b main
"
```

- [ ] **Step 1.2: Create directory structure**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
mkdir -p data labels features models training backtesting live configs tests notebooks docs
touch data/__init__.py labels/__init__.py features/__init__.py
touch models/__init__.py training/__init__.py backtesting/__init__.py live/__init__.py
touch tests/__init__.py
touch notebooks/.gitkeep docs/.gitkeep
"
```

- [ ] **Step 1.3: Write pyproject.toml**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/pyproject.toml << 'EOF'
[project]
name = \"trading-crypto\"
version = \"0.1.0\"
description = \"ML crypto trading pipeline: XGBoost signals + PPO portfolio agent\"
requires-python = \">=3.12\"
dependencies = [
    \"ccxt>=4.4.0\",
    \"pandas>=2.2.0\",
    \"pyarrow>=16.0.0\",
    \"requests>=2.32.0\",
    \"pandas-ta>=0.3.14b\",
    \"scikit-learn>=1.5.0\",
    \"xgboost>=2.1.0\",
    \"optuna>=3.6.0\",
    \"mlflow>=2.14.0\",
    \"pyyaml>=6.0.0\",
]

[project.optional-dependencies]
dev = [\"pytest>=8.0.0\", \"pytest-cov>=5.0.0\", \"responses>=0.25.0\"]

[build-system]
requires = [\"hatchling\"]
build-backend = \"hatchling.build\"

[tool.pytest.ini_options]
testpaths = [\"tests\"]
python_files = [\"test_*.py\"]

[tool.hatch.build.targets.wheel]
packages = [\"data\", \"labels\", \"features\", \"models\", \"training\", \"backtesting\", \"live\"]
EOF"
```

Note: `torch`, `finrl`, `stable-baselines3`, `pytorch-forecasting`, `freqtrade` are **deferred to Plan 2+**. The GB10 uses CUDA 13.0 which requires PyTorch nightly — that installation is handled separately when models are introduced. `xgboost` here is CPU-only for Plan 1 data pipeline validation; GPU flag added in Plan 2.

- [ ] **Step 1.4: Create virtual environment and install dependencies**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
/home/agents/.local/bin/uv venv --python 3.12 .venv
/home/agents/.local/bin/uv pip install -e '.[dev]'
"
```

Expected: resolves and installs without errors. May take 1-2 minutes.

- [ ] **Step 1.5: Write data_config.yaml**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/configs/data_config.yaml << 'EOF'
exchange: binance
timeframe: 4h
assets:
  - BTC/USDT
  - ETH/USDT

data_dir: /home/agents/trading-crypto/data/raw

onchain:
  publish_lag_hours:
    blockchain_btc_active_addresses: 24
    etherscan_eth_active_addresses: 24
    coingecko_exchange_volume: 12
  etherscan_api_key: \"\"  # set via env var ETHERSCAN_API_KEY

history_days: 730  # 2 years
EOF"
```

- [ ] **Step 1.6: Write README.md**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/README.md << 'EOF'
# trading-crypto

ML crypto trading pipeline for SGDGX01 (NVIDIA Grace Blackwell GB10).

## Architecture

- **Phase 1:** XGBoost directional signal classifier (up/flat/down) for BTC/USDT + ETH/USDT spot
- **Phase 2:** PPO RL portfolio agent (FinRL) built on Phase 1 feature pipeline
- **Live execution:** FreqAI (Freqtrade) imports shared feature library for live Binance spot trading

## Setup

\`\`\`bash
cd /home/agents/trading-crypto
/home/agents/.local/bin/uv venv --python 3.12 .venv
source .venv/bin/activate
uv pip install -e '.[dev]'
\`\`\`

## Fetch data

\`\`\`bash
python -m data.fetcher
\`\`\`

## Run tests

\`\`\`bash
pytest tests/ -v
\`\`\`

## Plans

- Plan 1 (this): Foundation — data pipeline, validation, on-chain, labels
- Plan 2: Feature engineering + XGBoost classifier + Optuna/MLflow
- Plan 3: Backtesting (CPCV, costs, metrics)
- Plan 4: FreqAI live strategy + drift monitoring
EOF"
```

- [ ] **Step 1.7: Write .gitignore**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/.gitignore << 'EOF'
.venv/
__pycache__/
*.pyc
*.pyo
.pytest_cache/
.coverage
htmlcov/
data/raw/
mlruns/
*.parquet
*.pkl
*.pt
.env
EOF"
```

- [ ] **Step 1.8: Initial commit**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
git add -A
git commit -m 'chore: initial project scaffold'
"
```

Expected: commit succeeds, `main` branch exists.

---

## Task 2: Data Exceptions + Fetcher

**Files:**
- Create: `data/exceptions.py`
- Create: `data/fetcher.py`
- Create: `tests/test_fetcher.py`

- [ ] **Step 2.1: Write failing tests for fetcher**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/tests/test_fetcher.py << 'EOF'
from __future__ import annotations
import pytest
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import MagicMock, patch
import pandas as pd
import tempfile

from data.fetcher import fetch_ohlcv, save_ohlcv, load_latest_ohlcv

# Shared fixture: 3 fake 4h OHLCV rows from Binance (ms timestamps)
FAKE_ROWS = [
    [1700000000000, 37000.0, 37500.0, 36800.0, 37200.0, 100.0],
    [1700014400000, 37200.0, 37600.0, 37100.0, 37400.0, 120.0],
    [1700028800000, 37400.0, 37800.0, 37300.0, 37600.0, 110.0],
]


@patch(\"ccxt.binance\")
def test_fetch_ohlcv_returns_dataframe(mock_binance_cls):
    mock_exchange = MagicMock()
    mock_exchange.rateLimit = 0
    mock_exchange.fetch_ohlcv.side_effect = [FAKE_ROWS, []]
    mock_binance_cls.return_value = mock_exchange

    df = fetch_ohlcv(\"BTC/USDT\", timeframe=\"4h\")

    assert isinstance(df, pd.DataFrame)
    assert list(df.columns) == [\"open\", \"high\", \"low\", \"close\", \"volume\"]
    assert len(df) == 3
    assert df.index.name == \"timestamp\"
    assert df.index.tz is not None  # UTC-aware


@patch(\"ccxt.binance\")
def test_fetch_ohlcv_index_is_sorted(mock_binance_cls):
    mock_exchange = MagicMock()
    mock_exchange.rateLimit = 0
    # Return rows in wrong order to test sorting
    mock_exchange.fetch_ohlcv.side_effect = [list(reversed(FAKE_ROWS)), []]
    mock_binance_cls.return_value = mock_exchange

    df = fetch_ohlcv(\"BTC/USDT\")
    assert df.index.is_monotonic_increasing


def test_save_and_load_roundtrip():
    df = pd.DataFrame(
        {\"open\": [1.0], \"high\": [2.0], \"low\": [0.5], \"close\": [1.5], \"volume\": [100.0]},
        index=pd.DatetimeIndex([pd.Timestamp(\"2024-01-01\", tz=\"UTC\")], name=\"timestamp\"),
    )
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp)
        fetch_ts = datetime(2024, 1, 2, tzinfo=timezone.utc)
        save_ohlcv(df, \"BTC/USDT\", path, fetch_ts)
        loaded = load_latest_ohlcv(\"BTC/USDT\", path)
        assert loaded is not None
        pd.testing.assert_frame_equal(df, loaded)


def test_load_latest_returns_none_when_no_files():
    with tempfile.TemporaryDirectory() as tmp:
        result = load_latest_ohlcv(\"BTC/USDT\", Path(tmp))
        assert result is None


@patch(\"ccxt.binance\")
def test_fetch_ohlcv_paginates_until_empty(mock_binance_cls):
    mock_exchange = MagicMock()
    mock_exchange.rateLimit = 0
    # First call returns 2 rows, second call returns empty (done)
    mock_exchange.fetch_ohlcv.side_effect = [FAKE_ROWS[:2], []]
    mock_binance_cls.return_value = mock_exchange

    df = fetch_ohlcv(\"BTC/USDT\", limit=2)
    assert len(df) == 2
    assert mock_exchange.fetch_ohlcv.call_count == 2
EOF"
```

- [ ] **Step 2.2: Run tests — expect failure**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
source .venv/bin/activate
pytest tests/test_fetcher.py -v 2>&1 | tail -20
"
```

Expected: `ModuleNotFoundError: No module named 'data.fetcher'` or similar — confirms tests are wired correctly.

- [ ] **Step 2.3: Write data/exceptions.py**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/data/exceptions.py << 'EOF'
class DataValidationError(Exception):
    \"\"\"Raised when OHLCV data fails invariant checks.\"\"\"
EOF"
```

- [ ] **Step 2.4: Write data/fetcher.py**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/data/fetcher.py << 'EOF'
from __future__ import annotations
import time
from datetime import datetime, timezone
from pathlib import Path

import ccxt
import pandas as pd

COLUMNS = [\"timestamp\", \"open\", \"high\", \"low\", \"close\", \"volume\"]


def fetch_ohlcv(
    asset: str,
    timeframe: str = \"4h\",
    since: datetime | None = None,
    exchange_id: str = \"binance\",
    limit: int = 1000,
) -> pd.DataFrame:
    \"\"\"Fetch OHLCV from exchange via ccxt, paginating until no more data.\"\"\"
    exchange = getattr(ccxt, exchange_id)({\"enableRateLimit\": True})
    since_ms = int(since.timestamp() * 1000) if since else None
    rows: list = []

    while True:
        batch = exchange.fetch_ohlcv(asset, timeframe=timeframe, since=since_ms, limit=limit)
        if not batch:
            break
        rows.extend(batch)
        if len(batch) < limit:
            break
        since_ms = batch[-1][0] + 1
        time.sleep(exchange.rateLimit / 1000)

    df = pd.DataFrame(rows, columns=COLUMNS)
    df[\"timestamp\"] = pd.to_datetime(df[\"timestamp\"], unit=\"ms\", utc=True)
    df = df.set_index(\"timestamp\").sort_index()
    return df


def save_ohlcv(df: pd.DataFrame, asset: str, data_dir: Path, fetch_ts: datetime) -> Path:
    \"\"\"Save OHLCV snapshot to Parquet. Filename encodes fetch timestamp for revision tracking.\"\"\"
    data_dir.mkdir(parents=True, exist_ok=True)
    safe_asset = asset.replace(\"/\", \"_\")
    fetch_str = fetch_ts.strftime(\"%Y%m%dT%H%M%S\")
    path = data_dir / f\"{safe_asset}_{fetch_str}.parquet\"
    df.to_parquet(path, engine=\"pyarrow\", compression=\"snappy\")
    return path


def load_latest_ohlcv(asset: str, data_dir: Path) -> pd.DataFrame | None:
    \"\"\"Load the most recent Parquet snapshot for an asset. Returns None if none exist.\"\"\"
    safe_asset = asset.replace(\"/\", \"_\")
    files = sorted(data_dir.glob(f\"{safe_asset}_*.parquet\"))
    if not files:
        return None
    return pd.read_parquet(files[-1], engine=\"pyarrow\")
EOF"
```

- [ ] **Step 2.5: Run tests — expect pass**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
source .venv/bin/activate
pytest tests/test_fetcher.py -v
"
```

Expected output:
```
tests/test_fetcher.py::test_fetch_ohlcv_returns_dataframe PASSED
tests/test_fetcher.py::test_fetch_ohlcv_index_is_sorted PASSED
tests/test_fetcher.py::test_save_and_load_roundtrip PASSED
tests/test_fetcher.py::test_load_latest_returns_none_when_no_files PASSED
tests/test_fetcher.py::test_fetch_ohlcv_paginates_until_empty PASSED
5 passed
```

- [ ] **Step 2.6: Commit**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
git add data/exceptions.py data/fetcher.py tests/test_fetcher.py
git commit -m 'feat: OHLCV fetcher with ccxt pagination and Parquet snapshots'
"
```

---

## Task 3: Data Validation

**Files:**
- Create: `data/validate.py`
- Create: `tests/test_validate.py`

- [ ] **Step 3.1: Write failing tests for validation**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/tests/test_validate.py << 'EOF'
from __future__ import annotations
import pytest
import pandas as pd
import numpy as np
from datetime import timezone

from data.validate import validate_ohlcv
from data.exceptions import DataValidationError


def make_df(n: int = 5, freq: str = \"4h\") -> pd.DataFrame:
    \"\"\"Create a valid OHLCV DataFrame with n rows at given frequency.\"\"\"
    idx = pd.date_range(\"2024-01-01\", periods=n, freq=freq, tz=\"UTC\", name=\"timestamp\")
    return pd.DataFrame(
        {
            \"open\":   [100.0] * n,
            \"high\":   [105.0] * n,
            \"low\":    [95.0]  * n,
            \"close\":  [102.0] * n,
            \"volume\": [500.0] * n,
        },
        index=idx,
    )


def test_valid_dataframe_passes():
    validate_ohlcv(make_df())  # Should not raise


def test_empty_dataframe_raises():
    with pytest.raises(DataValidationError, match=\"empty\"):
        validate_ohlcv(make_df(0))


def test_non_monotonic_timestamps_raises():
    df = make_df(3)
    df = df.iloc[[2, 0, 1]]  # Shuffle rows
    with pytest.raises(DataValidationError, match=\"monoton\"):
        validate_ohlcv(df)


def test_duplicate_timestamps_raises():
    df = make_df(3)
    df = pd.concat([df, df.iloc[[0]]])
    with pytest.raises(DataValidationError, match=\"[Dd]uplicate\"):
        validate_ohlcv(df)


def test_gap_detection_raises():
    # First 3 rows at 4h, then a 3-day gap
    df1 = make_df(3, freq=\"4h\")
    df2 = make_df(2, freq=\"4h\")
    df2.index = df2.index + pd.Timedelta(days=3)
    df = pd.concat([df1, df2])
    with pytest.raises(DataValidationError, match=\"gap\"):
        validate_ohlcv(df, expected_gap_minutes=240)


def test_high_below_open_raises():
    df = make_df(3)
    df.iloc[1, df.columns.get_loc(\"high\")] = 90.0  # high < open (100)
    with pytest.raises(DataValidationError, match=\"high\"):
        validate_ohlcv(df)


def test_low_above_close_raises():
    df = make_df(3)
    df.iloc[1, df.columns.get_loc(\"low\")] = 110.0  # low > close (102)
    with pytest.raises(DataValidationError, match=\"low\"):
        validate_ohlcv(df)


def test_zero_volume_raises():
    df = make_df(3)
    df.iloc[1, df.columns.get_loc(\"volume\")] = 0.0
    with pytest.raises(DataValidationError, match=\"[Vv]olume\"):
        validate_ohlcv(df)
EOF"
```

- [ ] **Step 3.2: Run tests — expect failure**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
source .venv/bin/activate
pytest tests/test_validate.py -v 2>&1 | tail -10
"
```

Expected: `ModuleNotFoundError: No module named 'data.validate'`

- [ ] **Step 3.3: Write data/validate.py**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/data/validate.py << 'EOF'
from __future__ import annotations
import pandas as pd
from data.exceptions import DataValidationError


def validate_ohlcv(df: pd.DataFrame, expected_gap_minutes: int = 240) -> None:
    \"\"\"
    Validate an OHLCV DataFrame. Raises DataValidationError on any violation.
    Call this before passing data to any downstream module.
    \"\"\"
    if df.empty:
        raise DataValidationError(\"DataFrame is empty\")

    if not df.index.is_monotonic_increasing:
        raise DataValidationError(\"Timestamps are not monotonically increasing\")

    dups = df.index.duplicated()
    if dups.any():
        raise DataValidationError(f\"Duplicate timestamps: {df.index[dups].tolist()}\")

    if len(df) > 1:
        gaps_minutes = df.index.to_series().diff().dt.total_seconds().div(60).dropna()
        bad = gaps_minutes[gaps_minutes > expected_gap_minutes * 1.5]
        if not bad.empty:
            raise DataValidationError(f\"Unexpected gap(s) at: {bad.index.tolist()}\")

    if not (df[\"high\"] >= df[\"open\"]).all():
        raise DataValidationError(\"high < open detected\")
    if not (df[\"high\"] >= df[\"close\"]).all():
        raise DataValidationError(\"high < close detected\")
    if not (df[\"low\"] <= df[\"open\"]).all():
        raise DataValidationError(\"low > open detected\")
    if not (df[\"low\"] <= df[\"close\"]).all():
        raise DataValidationError(\"low > close detected\")
    if not (df[\"low\"] <= df[\"high\"]).all():
        raise DataValidationError(\"low > high detected\")

    if not (df[\"volume\"] > 0).all():
        bad_idx = df.index[df[\"volume\"] <= 0].tolist()
        raise DataValidationError(f\"Volume <= 0 at: {bad_idx}\")
EOF"
```

- [ ] **Step 3.4: Run tests — expect pass**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
source .venv/bin/activate
pytest tests/test_validate.py -v
"
```

Expected:
```
8 passed
```

- [ ] **Step 3.5: Commit**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
git add data/validate.py tests/test_validate.py
git commit -m 'feat: OHLCV validation with OHLC invariants and gap detection'
"
```

---

## Task 4: On-Chain Data Module

**Files:**
- Create: `data/onchain.py`
- Create: `tests/test_onchain.py`

- [ ] **Step 4.1: Write failing tests for on-chain module**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/tests/test_onchain.py << 'EOF'
from __future__ import annotations
import pytest
import pandas as pd
from datetime import datetime, timezone
from unittest.mock import patch

from data.onchain import (
    apply_publish_lag,
    broadcast_daily_to_4h,
    fetch_btc_active_addresses,
    fetch_sopr_stub,
    fetch_exchange_netflow_stub,
)


def make_daily_df(n: int = 5, col: str = \"value\") -> pd.DataFrame:
    idx = pd.date_range(\"2024-01-01\", periods=n, freq=\"D\", tz=\"UTC\", name=\"timestamp\")
    return pd.DataFrame({col: range(n)}, index=idx, dtype=float)


def test_apply_publish_lag_shifts_index_forward():
    df = make_daily_df(3)
    lagged = apply_publish_lag(df, publish_lag_hours=24)
    # Each timestamp should be shifted forward by 24h
    expected_first = pd.Timestamp(\"2024-01-02\", tz=\"UTC\")
    assert lagged.index[0] == expected_first


def test_apply_publish_lag_does_not_mutate_original():
    df = make_daily_df(3)
    original_first = df.index[0]
    apply_publish_lag(df, publish_lag_hours=24)
    assert df.index[0] == original_first  # Original unchanged


def test_broadcast_daily_to_4h_forward_fills():
    # Daily on-chain value, broadcast to 4h candles
    daily = make_daily_df(3, col=\"btc_active_addresses\")
    candle_idx = pd.date_range(\"2024-01-01\", periods=18, freq=\"4h\", tz=\"UTC\", name=\"timestamp\")
    result = broadcast_daily_to_4h(daily, candle_idx)
    assert len(result) == 18
    # Value from 2024-01-01 should appear in candles 0..5 (6 × 4h = 1 day)
    assert result[\"btc_active_addresses\"].iloc[0] == 0.0
    assert result[\"btc_active_addresses\"].iloc[5] == 0.0
    assert result[\"btc_active_addresses\"].iloc[6] == 1.0


def test_broadcast_daily_to_4h_returns_no_future_data():
    # If candle index is entirely before the daily data, result is all NaN
    daily = make_daily_df(3, col=\"btc_active_addresses\")
    daily.index = daily.index + pd.Timedelta(days=10)  # Future data
    candle_idx = pd.date_range(\"2024-01-01\", periods=6, freq=\"4h\", tz=\"UTC\", name=\"timestamp\")
    result = broadcast_daily_to_4h(daily, candle_idx)
    assert result[\"btc_active_addresses\"].isna().all()


@patch(\"data.onchain.requests.get\")
def test_fetch_btc_active_addresses_returns_dataframe(mock_get):
    mock_get.return_value.status_code = 200
    mock_get.return_value.raise_for_status = lambda: None
    mock_get.return_value.json.return_value = {
        \"values\": [
            {\"x\": 1704067200, \"y\": 850000},  # 2024-01-01 00:00 UTC
            {\"x\": 1704153600, \"y\": 870000},  # 2024-01-02 00:00 UTC
        ]
    }
    since = datetime(2024, 1, 1, tzinfo=timezone.utc)
    until = datetime(2024, 1, 3, tzinfo=timezone.utc)
    df = fetch_btc_active_addresses(since, until)
    assert \"btc_active_addresses\" in df.columns
    assert len(df) == 2
    assert df.index.name == \"timestamp\"


def test_fetch_sopr_stub_returns_nan_series():
    since = datetime(2024, 1, 1, tzinfo=timezone.utc)
    until = datetime(2024, 1, 5, tzinfo=timezone.utc)
    df = fetch_sopr_stub(since, until)
    assert \"sopr\" in df.columns
    assert df[\"sopr\"].isna().all()


def test_fetch_exchange_netflow_stub_returns_nan_series():
    since = datetime(2024, 1, 1, tzinfo=timezone.utc)
    until = datetime(2024, 1, 5, tzinfo=timezone.utc)
    df = fetch_exchange_netflow_stub(since, until, asset=\"btc\")
    assert \"btc_exchange_netflow\" in df.columns
    assert df[\"btc_exchange_netflow\"].isna().all()
EOF"
```

- [ ] **Step 4.2: Run tests — expect failure**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
source .venv/bin/activate
pytest tests/test_onchain.py -v 2>&1 | tail -10
"
```

Expected: `ModuleNotFoundError: No module named 'data.onchain'`

- [ ] **Step 4.3: Write data/onchain.py**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/data/onchain.py << 'EOF'
from __future__ import annotations
from datetime import datetime, timezone
import requests
import pandas as pd


def apply_publish_lag(df: pd.DataFrame, publish_lag_hours: int) -> pd.DataFrame:
    \"\"\"
    Shift the index forward by publish_lag_hours to represent when data becomes
    available. Prevents a candle at time T from using on-chain data not yet
    published at T.
    \"\"\"
    result = df.copy()
    result.index = result.index + pd.Timedelta(hours=publish_lag_hours)
    return result


def broadcast_daily_to_4h(daily_df: pd.DataFrame, ohlcv_index: pd.DatetimeIndex) -> pd.DataFrame:
    \"\"\"
    Forward-fill daily on-chain values onto a 4h candle index.
    Candles before the first available data point receive NaN.
    \"\"\"
    return daily_df.reindex(ohlcv_index, method=\"ffill\")


def fetch_btc_active_addresses(since: datetime, until: datetime) -> pd.DataFrame:
    \"\"\"Fetch daily BTC active address count from Blockchain.com (free, no API key).\"\"\"
    days = max(1, (until - since).days + 1)
    url = \"https://api.blockchain.info/charts/n-unique-addresses\"
    params = {
        \"timespan\": f\"{days}days\",
        \"start\": since.strftime(\"%Y-%m-%d\"),
        \"format\": \"json\",
        \"sampled\": \"true\",
    }
    resp = requests.get(url, params=params, timeout=30)
    resp.raise_for_status()
    values = resp.json().get(\"values\", [])
    df = pd.DataFrame(values)
    df[\"timestamp\"] = pd.to_datetime(df[\"x\"], unit=\"s\", utc=True)
    df = df.set_index(\"timestamp\")[[\"y\"]].rename(columns={\"y\": \"btc_active_addresses\"})
    return df.astype(float)


def fetch_eth_active_addresses(since: datetime, until: datetime, api_key: str) -> pd.DataFrame:
    \"\"\"
    Fetch daily ETH transaction count from Etherscan as proxy for active addresses.
    Free tier: https://etherscan.io/apis
    \"\"\"
    url = \"https://api.etherscan.io/api\"
    params = {
        \"module\": \"stats\",
        \"action\": \"dailytxfee\",
        \"startdate\": since.strftime(\"%Y-%m-%d\"),
        \"enddate\": until.strftime(\"%Y-%m-%d\"),
        \"sort\": \"asc\",
        \"apikey\": api_key,
    }
    resp = requests.get(url, params=params, timeout=30)
    resp.raise_for_status()
    data = resp.json().get(\"result\", [])
    df = pd.DataFrame(data)
    df[\"timestamp\"] = pd.to_datetime(df[\"UTCDate\"], utc=True)
    df = df.set_index(\"timestamp\")[[\"value\"]].rename(columns={\"value\": \"eth_tx_count\"})
    return df.astype(float)


# TODO: replace with Glassnode API (paid, ~$39/mo)
# Endpoint: https://api.glassnode.com/v1/metrics/indicators/sopr
def fetch_sopr_stub(since: datetime, until: datetime) -> pd.DataFrame:
    \"\"\"Stub — returns NaN. Replace with Glassnode API.\"\"\"
    idx = pd.date_range(since, until, freq=\"D\", tz=\"UTC\", name=\"timestamp\")
    return pd.DataFrame({\"sopr\": float(\"nan\")}, index=idx)


# TODO: replace with Glassnode API
# Endpoint: https://api.glassnode.com/v1/metrics/transactions/transfers_volume_exchanges_net
def fetch_exchange_netflow_stub(since: datetime, until: datetime, asset: str) -> pd.DataFrame:
    \"\"\"Stub — returns NaN. Replace with Glassnode API.\"\"\"
    idx = pd.date_range(since, until, freq=\"D\", tz=\"UTC\", name=\"timestamp\")
    return pd.DataFrame({f\"{asset}_exchange_netflow\": float(\"nan\")}, index=idx)
EOF"
```

- [ ] **Step 4.4: Run tests — expect pass**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
source .venv/bin/activate
pytest tests/test_onchain.py -v
"
```

Expected:
```
7 passed
```

- [ ] **Step 4.5: Commit**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
git add data/onchain.py tests/test_onchain.py
git commit -m 'feat: on-chain data module with publish lag, free APIs, and Glassnode stubs'
"
```

---

## Task 5: Triple-Barrier Labeling

**Files:**
- Create: `labels/triple_barrier.py`
- Create: `tests/test_labels.py`

- [ ] **Step 5.1: Write failing tests for triple-barrier**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/tests/test_labels.py << 'EOF'
from __future__ import annotations
import pytest
import numpy as np
import pandas as pd

from labels.triple_barrier import (
    compute_realized_volatility,
    apply_triple_barrier,
    apply_embargo,
)


def make_price_series(n: int = 50, seed: int = 42) -> pd.Series:
    rng = np.random.default_rng(seed)
    returns = rng.normal(0, 0.01, n)
    prices = 100.0 * np.exp(np.cumsum(returns))
    idx = pd.date_range(\"2024-01-01\", periods=n, freq=\"4h\", tz=\"UTC\", name=\"timestamp\")
    return pd.Series(prices, index=idx, name=\"close\")


def test_realized_volatility_returns_series():
    prices = make_price_series()
    vol = compute_realized_volatility(prices, window=20)
    assert isinstance(vol, pd.Series)
    assert len(vol) == len(prices)


def test_realized_volatility_first_window_is_nan():
    prices = make_price_series(50)
    vol = compute_realized_volatility(prices, window=20)
    # First 20 values should be NaN (not enough data for rolling)
    assert vol.iloc[:20].isna().all()
    assert not vol.iloc[20:].isna().all()


def test_realized_volatility_is_positive():
    prices = make_price_series()
    vol = compute_realized_volatility(prices, window=10)
    valid = vol.dropna()
    assert (valid > 0).all()


def test_triple_barrier_labels_are_valid_values():
    prices = make_price_series(100)
    vol = compute_realized_volatility(prices, window=10)
    labels = apply_triple_barrier(prices, vol, k=1.5, n_bars=10)
    valid = labels.dropna()
    assert set(valid.unique()).issubset({1.0, 0.0, -1.0})


def test_triple_barrier_last_n_bars_are_nan():
    n_bars = 10
    prices = make_price_series(60)
    vol = compute_realized_volatility(prices, window=10)
    labels = apply_triple_barrier(prices, vol, k=1.5, n_bars=n_bars)
    # Last n_bars cannot be labeled (barrier window extends past end of data)
    assert labels.iloc[-n_bars:].isna().all()


def test_triple_barrier_has_some_labeled_rows():
    prices = make_price_series(100)
    vol = compute_realized_volatility(prices, window=10)
    labels = apply_triple_barrier(prices, vol, k=1.5, n_bars=10)
    # Should have at least some non-NaN labels
    assert labels.notna().sum() > 10


def test_embargo_sets_last_n_to_nan():
    labels = pd.Series([1.0, 0.0, -1.0, 1.0, 0.0])
    embargoed = apply_embargo(labels, n_bars=2)
    assert embargoed.iloc[-2:].isna().all()
    assert not embargoed.iloc[:-2].isna().any()


def test_embargo_does_not_mutate_input():
    labels = pd.Series([1.0, -1.0, 0.0])
    original = labels.copy()
    apply_embargo(labels, n_bars=1)
    pd.testing.assert_series_equal(labels, original)
EOF"
```

- [ ] **Step 5.2: Run tests — expect failure**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
source .venv/bin/activate
pytest tests/test_labels.py -v 2>&1 | tail -10
"
```

Expected: `ModuleNotFoundError: No module named 'labels.triple_barrier'`

- [ ] **Step 5.3: Write labels/triple_barrier.py**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/labels/triple_barrier.py << 'EOF'
from __future__ import annotations
import numpy as np
import pandas as pd


def compute_realized_volatility(close: pd.Series, window: int = 20) -> pd.Series:
    \"\"\"Rolling realized volatility: std of log-returns over window periods.\"\"\"
    log_returns = np.log(close / close.shift(1))
    return log_returns.rolling(window).std()


def apply_triple_barrier(
    close: pd.Series,
    volatility: pd.Series,
    k: float = 1.5,
    n_bars: int = 10,
) -> pd.Series:
    \"\"\"
    Triple-barrier labeling (López de Prado, AFML ch.3).

    For each bar i, sets:
      - upper barrier: price[i] * (1 + k * vol[i])
      - lower barrier: price[i] * (1 - k * vol[i])
      - vertical barrier: i + n_bars

    Label:  1 if upper hit first
            -1 if lower hit first
             0 if vertical barrier hit first (timeout)
            NaN if volatility is NaN or last n_bars rows (no full window)

    Args:
        close: Close price series.
        volatility: Realized volatility series (same index as close).
        k: Barrier width multiplier (number of vol units).
        n_bars: Max number of bars to wait before vertical barrier triggers.

    Returns:
        Series of float labels (1.0, 0.0, -1.0, or NaN).
    \"\"\"
    labels = pd.Series(np.nan, index=close.index, dtype=float)

    for i in range(len(close) - n_bars):
        vol = volatility.iloc[i]
        if pd.isna(vol) or vol == 0:
            continue

        price = close.iloc[i]
        upper = price * (1.0 + k * vol)
        lower = price * (1.0 - k * vol)
        window = close.iloc[i + 1 : i + 1 + n_bars]

        upper_hits = window[window >= upper]
        lower_hits = window[window <= lower]

        upper_time = upper_hits.index[0] if not upper_hits.empty else None
        lower_time = lower_hits.index[0] if not lower_hits.empty else None

        if upper_time is None and lower_time is None:
            labels.iloc[i] = 0.0
        elif upper_time is None:
            labels.iloc[i] = -1.0
        elif lower_time is None:
            labels.iloc[i] = 1.0
        else:
            labels.iloc[i] = 1.0 if upper_time <= lower_time else -1.0

    return labels


def apply_embargo(labels: pd.Series, n_bars: int) -> pd.Series:
    \"\"\"
    Set the last n_bars labels to NaN. These rows have barrier windows that
    extend past the end of available data and cannot be validly labeled.
    \"\"\"
    result = labels.copy()
    result.iloc[-n_bars:] = np.nan
    return result
EOF"
```

- [ ] **Step 5.4: Run tests — expect pass**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
source .venv/bin/activate
pytest tests/test_labels.py -v
"
```

Expected:
```
8 passed
```

- [ ] **Step 5.5: Run the full test suite**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
source .venv/bin/activate
pytest tests/ -v --tb=short
"
```

Expected:
```
tests/test_fetcher.py   5 passed
tests/test_validate.py  8 passed
tests/test_onchain.py   7 passed
tests/test_labels.py    8 passed
28 passed
```

- [ ] **Step 5.6: Commit**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
git add labels/triple_barrier.py tests/test_labels.py
git commit -m 'feat: triple-barrier labeling with vol-scaled barriers and embargo'
"
```

---

## Task 6: Smoke Test — Fetch Real Data

A quick integration check to verify ccxt → validate → label works end-to-end with real Binance data. This is not part of the automated test suite (it makes real network calls).

- [ ] **Step 6.1: Create smoke test script**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/scripts/smoke_test_data.py << 'EOF'
\"\"\"
Smoke test: fetch 30 days of BTC/USDT 4h candles from Binance,
validate, label, and print a summary.
Run: python scripts/smoke_test_data.py
\"\"\"
from datetime import datetime, timedelta, timezone
from pathlib import Path
import sys
sys.path.insert(0, \"/home/agents/trading-crypto\")

from data.fetcher import fetch_ohlcv, save_ohlcv, load_latest_ohlcv
from data.validate import validate_ohlcv
from labels.triple_barrier import compute_realized_volatility, apply_triple_barrier, apply_embargo

DATA_DIR = Path(\"/home/agents/trading-crypto/data/raw\")
since = datetime.now(timezone.utc) - timedelta(days=30)

print(\"Fetching BTC/USDT 4h candles (last 30 days)...\")
df = fetch_ohlcv(\"BTC/USDT\", timeframe=\"4h\", since=since)
print(f\"  Fetched {len(df)} candles\")

print(\"Validating...\")
validate_ohlcv(df)
print(\"  Validation passed\")

print(\"Saving snapshot...\")
path = save_ohlcv(df, \"BTC/USDT\", DATA_DIR, datetime.now(timezone.utc))
print(f\"  Saved to {path}\")

print(\"Labeling...\")
vol = compute_realized_volatility(df[\"close\"], window=20)
labels = apply_triple_barrier(df[\"close\"], vol, k=1.5, n_bars=10)
labels = apply_embargo(labels, n_bars=10)
counts = labels.value_counts().to_dict()
print(f\"  Labels: up={counts.get(1.0, 0)}, flat={counts.get(0.0, 0)}, down={counts.get(-1.0, 0)}, NaN={labels.isna().sum()}\")
print(\"Smoke test passed.\")
EOF
mkdir -p /home/agents/trading-crypto/scripts"
```

- [ ] **Step 6.2: Run smoke test**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
source .venv/bin/activate
python scripts/smoke_test_data.py
"
```

Expected output (counts will vary):
```
Fetching BTC/USDT 4h candles (last 30 days)...
  Fetched ~180 candles
Validating...
  Validation passed
Saving snapshot...
  Saved to /home/agents/trading-crypto/data/raw/BTC_USDT_...parquet
Labeling...
  Labels: up=X, flat=Y, down=Z, NaN=30
Smoke test passed.
```

- [ ] **Step 6.3: Commit scripts directory**

```bash
ssh sgdgx01 "
cd /home/agents/trading-crypto
git add scripts/smoke_test_data.py
git commit -m 'chore: add smoke test script for data pipeline'
"
```

---

## Self-Review Checklist

**Spec coverage:**
- ✅ Project scaffold with uv + pyproject.toml (Task 1)
- ✅ ccxt OHLCV fetcher with Parquet snapshots and fetch_timestamp for revision tracking (Task 2)
- ✅ Data validation: monotonic timestamps, no duplicates, gap detection, OHLC invariants, volume > 0 (Task 3)
- ✅ On-chain: Blockchain.com BTC active addresses, Etherscan ETH tx count, publish_lag, daily→4h broadcast, SOPR stub, netflow stub with `# TODO: Glassnode` (Task 4)
- ✅ Triple-barrier labeling with vol-scaled barriers, embargo, 3-class output (Task 5)
- ✅ Integration smoke test with real Binance data (Task 6)
- ✅ `.gitignore` excludes raw Parquet data and venv
- ⏭️ `features/engineer.py` — Plan 2
- ⏭️ `models/xgb_classifier.py` — Plan 2
- ⏭️ MLflow + Optuna — Plan 2
- ⏭️ CPCV backtesting — Plan 3
- ⏭️ FreqAI strategy — Plan 4

**Type consistency:** `fetch_ohlcv` returns `pd.DataFrame`, accepted by `validate_ohlcv(df)`, `save_ohlcv(df, ...)`. `apply_triple_barrier` accepts `pd.Series` close and vol — both produced by the smoke test correctly. `apply_embargo` accepts output of `apply_triple_barrier` — ✅ consistent.

**No placeholders:** All test code is complete. All implementation code is complete. Glassnode TODOs are intentional per spec.
