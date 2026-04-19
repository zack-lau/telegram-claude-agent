# trading-crypto Phase 2a: PPO RL Environment + Training

**Date:** 2026-04-19
**Status:** Approved (post-Opus review)

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
- **SB3 directly** (not FinRL) — FinRL's portfolio env assumptions don't fit 4h crypto bars with Sharpe reward; SB3 gives full control. Pin `stable-baselines3>=2.0` (requires `gymnasium`, not legacy `gym`).
- **XGBoost probs in state** — 6 pre-digested directional values (3 classes × 2 assets) let PPO focus on portfolio-level decisions rather than re-learning technical analysis
- **Cost penalty in reward** — agent trained with fee+slippage so it learns to avoid churn; backtest Sharpe matches training Sharpe
- **Fixed 252-bar episodes** — randomly sampled from training period; simpler than CPCV-aligned episodes, captures diverse market regimes, upgradeable in Phase 2b

---

## 3. Environment (`models/ppo_env.py`)

### Observation Space

Flat float32 vector, 449 dimensions per step. **Explicit timing:** at step `t`, the observation contains only information available at or before the close of bar `t`. Signal carries over to earn return from `t → t+1`.

| Component | Size | Source | Timing |
|---|---|---|---|
| Feature window (K=10 × 22 features × 2 assets) | 440 | `features/engineer.py` + `RobustScaler` | bars `[t-K+1, t]` inclusive |
| XGBoost class probs BTC (up/flat/down) | 3 | Saved BTC XGBoost model | probs from features at bar `t` |
| XGBoost class probs ETH (up/flat/down) | 3 | Saved ETH XGBoost model | probs from features at bar `t` |
| Current portfolio weights (BTC, ETH, USDT) | 3 | Internal env state | weights after last rebalance |

All feature values z-scored via fitted `RobustScaler` (loaded from `models/saved/{asset}_scaler.joblib`). Observation dtype is `np.float32` (required by SB3 — not float64). XGBoost probs are already in [0,1].

**XGB prob generation:** probs are precomputed over the full dataset as `xgb.predict_proba(X_scaled)` using only features up to each bar. The XGB models were trained on the PPO train split only — but their probs on the PPO test split are technically in-sample for XGB (XGB was trained on the same period). This is a known mild bias: test-period XGB probs are slightly better-calibrated than they would be for a truly unseen regime. Accepted for Phase 2a; Phase 2b addresses this by XGB retraining on PPO-aligned splits.

### Action Space

`gymnasium.spaces.Box(low=-10, high=10, shape=(3,), dtype=np.float32)`

Wide bounds so softmax can saturate (allowing near-100% allocation to one asset). Raw logits output by the Gaussian policy are converted to valid allocation weights inside `step()` via softmax:

```python
weights = np.exp(action) / np.exp(action).sum()  # [w_btc, w_eth, w_usdt], sums to 1
```

Long-only by construction (softmax output is always positive). No short selling in Phase 2a.

### Reward

The model assumes **full rebalance every step** — weights are reset to `w_new` at each bar. This is the simplest correct interpretation: USDT earns 0 log-return, and both assets earn their next-bar returns weighted by the new allocation.

```python
# Portfolio return (next-bar, after rebalancing to w_new)
portfolio_return = w_new_btc * next_log_return_btc + w_new_eth * next_log_return_eth

# Transaction cost: full L1 turnover × (fee + slippage per side)
# sum(|Δw|) counts both sell-side and buy-side — no /2
turnover = sum(abs(w_new_i - w_prev_i) for i in [BTC, ETH, USDT])
transaction_cost = turnover * (fee_rate + slippage_bps / 10_000)

reward = portfolio_return - transaction_cost
```

`fee_rate=0.001` and `slippage_bps=10` match `backtesting/costs.py`. No `/2` — rotating 100% BTC → ETH incurs a sell fee AND a buy fee (two events), so full L1 turnover is correct.

### Episode Logic

- **Length:** 252 bars (≈6 weeks of 4h data), configurable via `episode_length` in config
- **Start:** randomly sampled from training period, requiring at least K=10 warmup bars before episode start
- **Initial weights:** sampled from a symmetric Dirichlet(α=1) distribution — avoids the bias of always starting at equal weight (which would cause large turnover on step 0 whenever the agent prefers a concentrated allocation)
- **Terminal condition:** episode ends at bar 252 (`truncated=True`); no early termination
- **`reset(*, seed=None, options=None)`:** full gymnasium API signature; respects SB3 seeding for reproducibility. Returns `(obs, info)` tuple.

### Interface

```python
class PortfolioPPOEnv(gymnasium.Env):
    def __init__(self, df_btc, df_eth, xgb_btc, xgb_eth, scaler_btc, scaler_eth, cfg)
    def reset(self, *, seed=None, options=None) -> tuple[np.ndarray, dict]
    def step(self, action: np.ndarray) -> tuple[np.ndarray, float, bool, bool, dict]
    # step returns: obs, reward, terminated, truncated, info
    # info dict: {"weights": w_new, "portfolio_return": float, "transaction_cost": float, "step": int}
```

`check_env(env)` from `stable_baselines3.common.env_checker` must pass before any training run — add to tests and CI.

---

## 4. Agent Wrapper (`models/ppo_agent.py`)

Thin wrapper around `stable_baselines3.PPO` — mirrors `XGBClassifierWrapper` interface:

```python
class PPOAgent:
    def train(self, env: PortfolioPPOEnv, total_timesteps: int, params: dict) -> None
    def predict(self, obs: np.ndarray, deterministic: bool = True) -> np.ndarray
        # returns allocation weights [btc, eth, usdt] after softmax (not raw SB3 action)
    def save(self, path: Path) -> None
    def load(self, path: Path) -> None
```

**SB3 configuration:**
- Policy: `MlpPolicy`
- Network architecture passed via `policy_kwargs={"net_arch": [256, 256, 256]}` — not as raw `n_layers`/`layer_size` keys (SB3 doesn't read those)
- `device="cuda"` — uses GB10 GPU
- Entropy coefficient: 0.01 (configurable)
- Reward normalization: wrap training env with `VecNormalize(norm_reward=True, norm_obs=False)` — SB3 PPO is sensitive to reward scale; 4h log returns (~10⁻³) would otherwise give near-zero advantage estimates
- All hyperparameters passed via `params` dict from `configs/ppo_config.yaml`
- CUDA determinism: `CUBLAS_WORKSPACE_CONFIG=:4096:8` env var set before training for reproducible results on GB10

**Promotion gate — multi-seed evaluation:**
Before saving, train N=5 seeds (configurable). Promote only if:
1. **Median test Sharpe across seeds > B&H Sharpe + 0.3** (margin guards against noise on a single test slice)
2. **Median test max drawdown < B&H max drawdown × 1.5** (agent must not blow up risk profile)
3. **Median turnover < 0.5 per step** (sanity check — if agent trades every bar it's gaming the reward, not learning)

Log all 5 seed results to MLflow. Save the median-seed model artifact.

---

## 5. Training Pipeline (`training/train_ppo.py`)

### Functions

```python
def build_ppo_dataset(data_dir: str) -> PPODataset
    # loads BTC + ETH, aligns timestamps, generates XGB probs, returns train/test split

def train_one_seed(dataset: PPODataset, params: dict, seed: int) -> tuple[PPOAgent, dict]
    # trains PPO for one seed, returns agent + eval metrics dict

def evaluate_agent(agent: PPOAgent, dataset: PPODataset) -> dict
    # runs single deterministic sequential episode on test split
    # returns: {sharpe, max_drawdown, mean_return, bh_sharpe, turnover_per_step, n_steps}

def main(data_dir: str) -> None
    # orchestrates: build → train N seeds → evaluate each → check promotion gate → MLflow
```

### Data Flow

1. Load BTC and ETH OHLCV via `build_dataset` (reuses Phase 1 pipeline) — returns `(X, y, next_log_returns, scaler)` per asset
2. Load saved XGBoost models from `models/saved/{asset}_xgb.ubj` and scalers
3. Generate XGBoost probability arrays: `xgb.predict_proba(X_scaled)` → shape `(n, 3)` per asset
4. Align BTC and ETH on timestamp index (inner join)
5. Train/test split: first 80% train, last 20% test, with `label_n_bars` embargo at boundary
6. Wrap train env with `VecNormalize(norm_reward=True, norm_obs=False)`
7. For each seed in range(N=5): train PPO for `total_timesteps`, evaluate on test
8. Check promotion gate across all seeds
9. If promoted: save median-seed model to `models/saved/btc_eth_ppo.zip`

### MLflow Schema

Every run logs:
- `feature_version` — from `features/engineer.py`
- `xgb_btc_run_id`, `xgb_eth_run_id` — which Phase 1 MLflow runs were used
- `seed_<i>_sharpe`, `seed_<i>_max_drawdown`, `seed_<i>_turnover` — per-seed results
- `median_test_sharpe`, `bh_sharpe`, `promoted` — aggregate
- `git_commit_sha`, `sb3_version`, `torch_version` — reproducibility
- All `ppo_config.yaml` values
- Artifact: `btc_eth_ppo.zip` + `vecnormalize.pkl` (if promoted)

### Retraining Cadence

Manual or cron-triggered. Must re-run when XGBoost models retrain. Tracked via `xgb_btc_run_id` / `xgb_eth_run_id` in MLflow.

---

## 6. Config (`configs/ppo_config.yaml`)

```yaml
# Training
total_timesteps: 500000
n_seeds: 5                   # seeds for promotion gate
episode_length: 252          # bars per training episode (~6 weeks of 4h)
k_window: 10                 # observation lookback

# Network (passed via policy_kwargs)
net_arch: [256, 256, 256]
learning_rate: 0.0003
entropy_coef: 0.01
gamma: 0.99
n_steps: 2048                # SB3 rollout buffer size
batch_size: 64

# Costs (must match backtesting/costs.py)
fee_rate: 0.001
slippage_bps: 10

# Hardware
device: cuda

# Promotion gate
promotion_sharpe_margin: 0.3       # median test Sharpe must exceed B&H by this
promotion_max_dd_multiplier: 1.5   # test MDD must be < B&H MDD × this
promotion_max_turnover_per_step: 0.5
```

---

## 7. Testing (`tests/test_ppo_env.py`)

| Test | What it verifies |
|---|---|
| `test_env_passes_check_env` | `check_env(env)` from SB3 passes (dtype, spaces, reset signature) |
| `test_env_reset_returns_correct_obs_shape` | Observation is 449-dim float32 |
| `test_env_step_weights_sum_to_one` | Softmax action → valid portfolio weights in [0,1] summing to 1 |
| `test_env_reward_penalizes_turnover` | High-churn action earns lower reward than hold on same return |
| `test_env_transaction_cost_no_division_by_two` | Full rotation BTC→ETH incurs 2× fee (not 1×) |
| `test_env_episode_ends_at_episode_length` | `truncated=True` at bar 252 |
| `test_env_deterministic_with_seed` | Same seed → same initial weights and episode start |
| `test_env_obs_timing_no_lookahead` | Obs at step t only uses features[t-K+1:t], reward uses log_return[t+1] |
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
| `pyproject.toml` | Modify — add torch, stable-baselines3>=2.0, gymnasium |

---

## 9. Dependencies

```
torch (CUDA 12.x)              # GPU training on GB10
stable-baselines3[extra]>=2.0  # PPO + gymnasium (not legacy gym)
gymnasium                       # env interface
```

---

## 10. What's Explicitly Out of Scope (Phase 2a)

- FreqAI live execution wiring
- Optuna hyperparameter tuning for PPO
- Vectorized multi-env training (`SubprocVecEnv`)
- Walk-forward RL evaluation (CPCV-aligned episodes)
- XGB retraining on PPO-aligned splits (known mild bias, deferred to Phase 2b)
- Short selling or leverage
- BNB or additional assets
- Longer observation windows (K=20+) — K=10 first, expand in Phase 2b if needed
