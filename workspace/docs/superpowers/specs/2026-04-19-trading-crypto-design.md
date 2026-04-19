# trading-crypto: Design Spec
**Date:** 2026-04-19
**Status:** Approved

---

## 1. Problem & Goal

Build a research-grade ML crypto trading pipeline that produces directional alpha signals for BTC and ETH spot trading, validated via rigorous walk-forward backtesting, and deployable to live markets via FreqAI (Freqtrade). A PPO reinforcement learning portfolio agent is the Phase 2 objective, built on top of the Phase 1 signal infrastructure.

**Not in scope:** Derivatives/perps, leverage, market-making, LLM sentiment, price regression.

---

## 2. Architecture

### Approach: Custom research pipeline + FreqAI live execution

The research pipeline (training, feature engineering, backtesting) is fully custom Python. FreqAI acts as the live execution layer. The critical design decision: `features/engineer.py` is a **shared library** imported by both the research pipeline and the FreqAI strategy — never duplicated. This is the primary defence against feature drift between research and live.

```
data/ ──► validate/ ──► labels/ ──► features/ ──► models/ ──► backtesting/
  │                                     │               │
ccxt (4h OHLCV)                  shared library     XGBoost       CPCV walk-forward
Dune/Blockchain.com              (one source        (Phase 1)     Sharpe, drawdown,
CoinGecko (on-chain)             of truth)          PPO           directional accuracy
publish_lag enforced                                (Phase 2)     tx cost model
                                     │
                              FreqAI strategy
                           (thin wrapper, imports
                            features/engineer.py)
                                     │
                              live spot execution
                           (Binance, risk controls,
                            vol-targeted sizing)
```

---

## 3. Assets & Timeframe

- **Assets:** BTC/USDT, ETH/USDT (BNB excluded from Phase 1 — exchange token with idiosyncratic Binance-specific risk; add in Phase 2 after pipeline validated)
- **Timeframe:** 4h candles
- **Exchange:** Binance spot via ccxt
- **History:** Minimum 2 years (~4,400 candles per asset)

---

## 4. Data Pipeline (`data/`)

### `fetcher.py`
- Pulls 4h OHLCV via ccxt from Binance
- Stores raw data as Parquet, keyed by `(asset, fetch_timestamp)` — snapshots preserved to avoid exchange revision bias
- Incremental fetching: only pulls candles newer than latest stored timestamp

### `validate.py`
- Enforces before any downstream use:
  - Monotonic timestamps, no duplicates
  - No gaps (or explicit gap flags — don't silently forward-fill)
  - OHLC invariants: `H >= max(O,C)`, `L <= min(O,C)`, `L <= H`
  - Volume > 0
  - Raises `DataValidationError` on violation; pipeline halts

### `onchain.py`
- **Implemented (free):**
  - BTC active addresses — Blockchain.com API
  - ETH active addresses — Etherscan API
  - Exchange netflow proxy — CoinGecko exchange volume delta
- **Stubs with `# TODO: replace with Glassnode API`:**
  - SOPR (BTC)
  - Exchange net inflow/outflow (BTC, ETH)
  - MVRV Z-score
- **Publication lag enforcement:** Each on-chain source has a `publish_lag_hours` config value. Features are aligned to candles using `candle_time >= source_publish_time - publish_lag`. Prevents lookahead bias from daily metrics bleeding into prior 4h candles.
- Daily metrics are broadcast to 4h via forward-fill with lag offset applied first.

---

## 5. Labeling (`labels/`)

### `triple_barrier.py`
- Implements López de Prado triple-barrier labeling (Advances in Financial ML, ch. 3)
- **Upper barrier:** `+k × σ` (volatility-scaled, rolling 20-period realized vol)
- **Lower barrier:** `-k × σ`
- **Vertical barrier:** max `N` candles (configurable, default 10 × 4h = ~1.7 days)
- Output: 3-class label {1=up, 0=flat, -1=down}
- **Embargo:** Last `N` candles of each training fold are dropped (no label assigned) to prevent barrier lookahead leakage
- `k` and `N` are Optuna-tunable hyperparameters, versioned in MLflow

---

## 6. Feature Engineering (`features/`)

### `engineer.py` — **The shared library**
Imported by research pipeline and FreqAI strategy. Single source of truth.

**Technical indicators (via pandas-ta):**
- RSI (14)
- MACD (12/26/9) — value, signal, histogram
- Bollinger Bands (20, 2σ) — upper, lower, %B, bandwidth
- EMA (9, 21, 50, 200)
- ATR (14) — for volatility scaling
- Volume z-score (20-period)

**On-chain features (from `data/onchain.py`, lag-adjusted):**
- Active addresses (normalized, 7d rolling z-score)
- Exchange netflow proxy (7d rolling z-score)
- SOPR stub (returns `NaN` until Glassnode integrated)

**Derived features:**
- Log-returns (1, 2, 6 periods)
- Rolling realized volatility (20-period)
- Cross-asset correlation (BTC↔ETH 20-period rolling)

**Feature versioning:**
- All features computed by `engineer.py` are tagged with `FEATURE_VERSION = "v1.0"` constant
- Version string written into every MLflow run and every FreqAI artifact export
- Changing any feature increments the version

### `scaler.py`
- Fits `RobustScaler` on training fold data only (never on full series)
- Persisted alongside model artifact, versioned
- Applied identically in research pipeline and FreqAI strategy

### `drift.py`
- Population Stability Index (PSI) per feature, computed weekly in live
- KS test on feature distributions vs training baseline
- Alerts (log + Telegram via Nicole bot) when PSI > 0.2 for any feature
- Halts live trading if > 30% of features exceed threshold

---

## 7. Models (`models/`)

### Phase 1: `xgb_classifier.py`
- XGBoost multi-class classifier (3 classes: up/flat/down)
- `tree_method="hist"`, `device="cuda"` for GB10
- One model per asset (BTC, ETH) — not cross-asset
- Inputs: feature vector from `features/engineer.py` + `scaler.py`
- Output: class probabilities; live signal uses `argmax` with confidence threshold ≥ 0.45 (configurable in `configs/xgb_config.yaml`)
- Hyperparameters tuned via Optuna (see Training)
- **Model promotion gate:** must beat buy-and-hold Sharpe on held-out CPCV fold before promoting to live

### Phase 2: `ppo_agent.py`
- FinRL PPO agent for BTC+ETH portfolio allocation
- State space: windowed feature matrix (last K=10 observations per asset) from same `features/engineer.py`
- Features z-scored and stationary (log-returns, not raw prices)
- Action space: continuous allocation weights across BTC, ETH, USDT cash
- Reward: Sharpe ratio of portfolio return over episode window
- `device = torch.device("cuda" if torch.cuda.is_available() else "cpu")`
- Built after Phase 1 signals are validated in live

---

## 8. Training (`training/`)

### `train_xgb.py`
- **Retraining cadence:** bi-weekly (every 14 days) by default; triggered manually or via cron on SGDGX01
- **Optuna study:** maximizes CPCV out-of-sample Sharpe across folds
- Hyperparameters tuned: `n_estimators`, `max_depth`, `learning_rate`, `subsample`, `colsample_bytree`, `min_child_weight`, label barrier `k` and `N`
- **Cap:** 50 trials max to prevent over-tuning to CV structure
- **Held-out final test set:** last 6 months of data, touched exactly once after Optuna completes
- Every trial logged to MLflow: hyperparams, RMSE, directional accuracy, Sharpe, max drawdown, feature_version, label_version

### `train_ppo.py`
- FinRL training loop with vectorized envs (GPU-accelerated)
- Same MLflow logging schema
- Phase 2 only

---

## 9. Backtesting (`backtesting/`)

### `engine.py`
- **Walk-forward with CPCV** (Combinatorial Purged Cross-Validation)
- Default: 6 folds, 4 used for training, 2 for test (configurable)
- Purging: embargo = max barrier horizon `N` candles between train/test
- Metrics per fold: Sharpe ratio, max drawdown, directional accuracy (% correct up/down/flat), win rate
- Aggregated across folds: mean + std of each metric
- **Directional accuracy baseline:** random = 33.3%; target > 40% to consider model viable

### `costs.py`
- Binance spot taker fee: 0.1% (or 0.075% with BNB fee discount — configurable)
- Slippage model: 5-15bps depending on asset and trade size (configurable)
- Applied to every simulated trade — no gross-return fantasy

---

## 10. Live Execution (`live/`)

### `freqai_strategy.py`
- Thin FreqAI strategy wrapper
- `populate_indicators()` calls `features.engineer.compute(dataframe)` — imports the shared library, does not reimplement
- `populate_entry_trend()` / `populate_exit_trend()` driven by model confidence threshold
- Disables FreqAI auto-retraining (model is retrained externally on schedule)
- Pinned FreqAI version in `pyproject.toml`

### `sizing.py`
- Volatility-targeted position sizing
- Target annualized portfolio vol: 15% (configurable)
- Per-trade stake = `(target_vol / asset_vol) × portfolio_value × max_allocation`
- Implemented via FreqAI's `custom_stake_amount` hook

---

## 11. Monitoring

- **Live-vs-backtest parity:** Live predictions + actual outcomes logged weekly; distribution compared to backtest
- **Feature drift:** `features/drift.py` runs weekly, alerts via Telegram (Nicole bot, SGDGX01)
- **Model drift:** Directional accuracy monitored rolling 30-day; drops below 52% trigger review
- **Risk kill switch:** Max daily drawdown 3%, max consecutive losses 5 → flatten + halt. FreqAI risk controls.

---

## 12. Project Structure

```
trading-crypto/
├── data/
│   ├── fetcher.py
│   ├── onchain.py
│   └── validate.py
├── labels/
│   └── triple_barrier.py
├── features/
│   ├── engineer.py         # shared library — single source of truth
│   ├── scaler.py
│   └── drift.py
├── models/
│   ├── xgb_classifier.py
│   └── ppo_agent.py
├── training/
│   ├── train_xgb.py
│   └── train_ppo.py
├── backtesting/
│   ├── engine.py
│   └── costs.py
├── live/
│   ├── freqai_strategy.py
│   └── sizing.py
├── configs/
│   ├── xgb_config.yaml
│   └── ppo_config.yaml
├── tests/
│   ├── test_feature_parity.py   # research == FreqAI features, byte-identical
│   ├── test_validate.py
│   └── test_labels.py
├── notebooks/
├── docs/
│   └── superpowers/specs/
└── pyproject.toml
```

---

## 13. Dependencies (`pyproject.toml`, managed by uv)

```
torch (CUDA 12.x)
xgboost[gpu]
pytorch-forecasting          # available for Phase 2 TFT exploration
finrl
freqtrade
ccxt
pandas-ta
scikit-learn
optuna
mlflow
pytest
requests                     # on-chain API calls
pyarrow                      # Parquet
stable-baselines3            # PPO backend for FinRL
```

---

## 14. Key Design Decisions & Rationale

| Decision | Rationale |
|---|---|
| Directional classification, not price regression | 2025 practitioner consensus; more robust, actionable signal |
| 4h candles | Enough data for XGBoost (~4,400 candles/2yr), on-chain aligns, less noise than 1h |
| BTC+ETH only (Phase 1) | BNB has idiosyncratic Binance-event risk; validate pipeline on clean assets first |
| Shared `features/engineer.py` | Prevents research/live feature drift — the #1 failure mode for this architecture |
| Triple-barrier labeling | Volatility-scaled; flat class is meaningful across regimes |
| CPCV not simple train/test | Prevents lookahead bias; required for credible backtest results |
| FreqAI for live execution | Battle-tested, built-in risk management, not worth reinventing |
| 50 Optuna trial cap | Prevents over-fitting to CV structure itself |
| Held-out final test touched once | Protects against researcher overfitting |
| Publish lag on on-chain data | Prevents inflated backtest Sharpe from timing leakage |

---

## 15. What's Explicitly Out of Scope (Phase 1)

- BNB trading
- Derivatives / perpetuals / leverage
- LLM/FinBERT sentiment features
- TFT / PatchTST models (no on-chain benchmarks exist; revisit in Phase 2)
- LSTM component of LSTM+XGBoost hybrid (add in Phase 1.5 if XGBoost alone validates)
- Order book / LOB features
- Multi-exchange arbitrage
