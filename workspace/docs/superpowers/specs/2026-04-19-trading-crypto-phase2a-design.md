# trading-crypto Phase 2a: PPO RL Environment + Training

**Date:** 2026-04-19
**Status:** Approved

---

## 1. Problem & Goal

Build a PPO reinforcement learning portfolio agent that allocates capital across BTC, ETH, and USDT cash using Phase 1 XGBoost signals and raw features as its state space. Phase 2a covers the RL environment, agent wrapper, and training pipeline only. Live execution wiring and walk-forward RL evaluation are Phase 2b.

**Not in scope (Phase 2a):** FreqAI live wiring, Optuna hyperparameter tuning, vectorized multi-env training, walk-forward RL backtesting.

---

## 2. Architecture

```
features/engineer.py  ─────────────────────────────────────────────┐
models/xgb_classifier.py (saved BTC + ETH models) ─────────────────┤
                                                                     ▼
                                                          models/ppo_env.py
                                                          (gymnasium.Env)
                                                                     │
                                                          models/ppo_agent.py
                                                          (SB3 PPO wrapper)
                                                                     │
                                                          training/train_ppo.py
                                                          (data prep, train,
                                                           evaluate, MLflow,
                                                           promotion gate)
```

**Key decisions:**
- **SB3 directly** (not FinRL) — FinRL's portfolio env assumptions don't fit 4h crypto bars with Sharpe reward; SB3 gives full control
- **XGBoost probs in state** — 6 pre-digested directional values (3 classes × 2 assets) let PPO focus on portfolio-level decisions rather than re-learning technical analysis
- **Cost penalty in reward** — agent trained with fee+slippage so it learns to avoid churn; backtest Sharpe matches training Sharpe
- **Fixed 252-bar episodes** — randomly sampled from training period; simpler than CPCV-aligned episodes, captures diverse market regimes, upgradeable in Phase 2b

---

## 3. Environment (`models/ppo_env.py`)

### Observation Space

Flat float32 vector, 449 dimensions per step:

| Component | Size | Source |
|---|---|---|
| Feature window (K=10 × 22 features × 2 assets) | 440 | `features/engineer.py` + `features/scaler.py` |
| XGBoost class probs (BTC: up/flat/down) | 3 | Saved BTC XGBoost model |
| XGBoost class probs (ETH: up/flat/down) | 3 | Saved ETH XGBoost model |
| Current portfolio weights (BTC, ETH, USDT) | 3 | Internal env state |

All feature values z-scored via fitted `RobustScaler` (loaded from `models/saved/{asset}_scaler.joblib`). XGBoost probs are already in [0,1], no additional scaling needed.

### Action Space

`gymnasium.spaces.Box(low=-1, high=1, shape=(3,), dtype=np.float32)`

Raw logits output by the policy network. Converted to valid allocation weights inside `step()` via softmax:
```python
weights = np.exp(action) / np.exp(action).sum()  # [w_btc, w_eth, w_usdt], sums to 1
```

Long-only by construction (softmax output is always positive). No short selling in Phase 2a.

### Reward

Single-step portfolio log-return after transaction costs:

```python
portfolio_return = sum(w_i * next_log_return_i for i in [BTC, ETH])
# USDT leg earns 0 return
turnover = sum(abs(w_new_i - w_prev_i) for i in [BTC, ETH, USDT]) / 2
transaction_cost = turnover * (fee_rate + slippage_bps / 10_000)
reward = portfolio_return - transaction_cost
```

Transaction cost is computed directly in the env as turnover × (fee + slippage). `fee_rate=0.001` and `slippage_bps=10` match the values in `backtesting/costs.py` and are configurable in `configs/ppo_config.yaml`. The `/2` avoids double-counting: selling BTC and buying ETH is one round-trip, not two separate trades.

### Episode Logic

- **Length:** 252 bars (≈6 weeks of 4h data), configurable via `episode_length` in config
- **Start:** randomly sampled from training period, requiring at least K=10 warmup bars before episode start
- **Initial weights:** equal allocation (BTC=1/3, ETH=1/3, USDT=1/3)
- **Terminal condition:** episode ends at bar 252; no early termination
- **`reset(seed=...)`:** respects SB3's seeding protocol for reproducibility

### Interface

```python
class PortfolioPPOEnv(gymnasium.Env):
    def __init__(self, df_btc, df_eth, xgb_btc, xgb_eth, scaler_btc, scaler_eth, cfg)
    def reset(self, seed=None) -> tuple[np.ndarray, dict]
    def step(self, action) -> tuple[np.ndarray, float, bool, bool, dict]
    # info dict includes: weights, portfolio_return, transaction_cost, step
```

---

## 4. Agent Wrapper (`models/ppo_agent.py`)

Thin wrapper around `stable_baselines3.PPO` — mirrors `XGBClassifierWrapper` interface:

```python
class PPOAgent:
    def train(self, env: PortfolioPPOEnv, total_timesteps: int, params: dict) -> None
    def predict(self, obs: np.ndarray) -> np.ndarray  # returns weights [btc, eth, usdt]
    def save(self, path: Path) -> None
    def load(self, path: Path) -> None
```

**SB3 configuration:**
- Policy: `MlpPolicy`
- Network: 3 hidden layers × 256 units (configurable)
- `device="cuda"` — uses GB10 GPU
- Entropy coefficient: 0.01 (encourages exploration, prevents premature convergence)
- All hyperparameters passed via `params` dict from `configs/ppo_config.yaml`

No Optuna tuning in Phase 2a — fixed hyperparameters validated first.

---

## 5. Training Pipeline (`training/train_ppo.py`)

### Data Flow

1. Load BTC and ETH OHLCV via `build_dataset` (reuses Phase 1 pipeline) — returns `(X, y, next_log_returns, scaler)`
2. Load saved XGBoost models from `models/saved/{asset}_xgb.ubj` and scalers from `models/saved/{asset}_scaler.joblib`
3. Generate XGBoost probability arrays over full history: `xgb.predict_proba(X)` → shape `(n, 3)`
4. Align BTC and ETH dataframes on timestamp index
5. Train/test split: first 80% train, last 20% test, with `label_n_bars` embargo at boundary (same as `train_xgb.py`)
6. Instantiate `PortfolioPPOEnv` on train split
7. Train PPO for `total_timesteps` (default 500,000)
8. Evaluate on test split: single sequential episode (no random start, no episode resets)
9. **Promotion gate:** test Sharpe > B&H Sharpe on test period → save to `models/saved/btc_eth_ppo.zip`; otherwise log warning and exit without saving
10. MLflow logging throughout

### Functions

```python
def build_ppo_dataset(data_dir: str) -> PPODataset
    # returns aligned BTC+ETH features, returns, xgb_probs, scalers, train/test split

def evaluate_agent(agent: PPOAgent, env: PortfolioPPOEnv) -> dict
    # runs single sequential test episode, returns:
    # {sharpe, max_drawdown, mean_return, buy_and_hold_sharpe, total_steps}

def main(data_dir: str) -> None
    # orchestrates build → train → evaluate → promote → MLflow
```

### MLflow Schema

Every run logs:
- `feature_version` — from `features/engineer.py`
- `xgb_btc_model_path`, `xgb_eth_model_path` — which Phase 1 models were used
- All `ppo_config.yaml` values as params
- Metrics: `train_sharpe`, `test_sharpe`, `test_max_drawdown`, `test_mean_return`, `bh_sharpe`, `promoted` (bool)
- Artifact: `btc_eth_ppo.zip` (if promoted)

### Retraining Cadence

Manual or cron-triggered. Must be re-run when XGBoost models retrain (probability distributions shift). Tracked via `xgb_btc_model_path` / `xgb_eth_model_path` in MLflow — if these change, PPO is stale.

---

## 6. Config (`configs/ppo_config.yaml`)

```yaml
# Training
total_timesteps: 500000
episode_length: 252          # bars per training episode (~6 weeks of 4h)
k_window: 10                 # observation lookback

# Network
n_layers: 3
layer_size: 256
learning_rate: 0.0003
entropy_coef: 0.01
gamma: 0.99
n_steps: 2048                # SB3 rollout buffer size
batch_size: 64

# Costs
fee_rate: 0.001
slippage_bps: 10

# Hardware
device: cuda

# Promotion
promotion_min_sharpe_vs_bh: 0.0   # test Sharpe must exceed B&H Sharpe
```

---

## 7. Testing (`tests/test_ppo_env.py`)

| Test | What it verifies |
|---|---|
| `test_env_reset_returns_correct_obs_shape` | Observation is 449-dim float32 |
| `test_env_step_weights_sum_to_one` | Softmax action → valid portfolio weights |
| `test_env_reward_penalizes_turnover` | High-churn action earns lower reward than hold |
| `test_env_episode_ends_at_episode_length` | `terminated=True` at bar 252 |
| `test_env_deterministic_with_seed` | Same seed → same episode trajectory |
| `test_ppo_agent_predict_shape` | `predict()` returns shape (3,) in [0,1] summing to 1 |

---

## 8. File Map

| File | Status |
|---|---|
| `models/ppo_env.py` | New |
| `models/ppo_agent.py` | New |
| `training/train_ppo.py` | New |
| `configs/ppo_config.yaml` | New |
| `tests/test_ppo_env.py` | New |
| `pyproject.toml` | Modify — add torch, stable-baselines3, gymnasium |

---

## 9. Dependencies

```
torch (CUDA 12.x)           # GPU training on GB10
stable-baselines3[extra]    # PPO implementation
gymnasium                   # env interface (SB3 compatible)
```

FinRL remains in `pyproject.toml` as a future reference but is not used in Phase 2a.

---

## 10. What's Explicitly Out of Scope (Phase 2a)

- FreqAI live execution wiring
- Optuna hyperparameter tuning for PPO
- Vectorized multi-env training (`SubprocVecEnv`)
- Walk-forward RL evaluation (CPCV-aligned episodes)
- Short selling or leverage
- BNB or additional assets
