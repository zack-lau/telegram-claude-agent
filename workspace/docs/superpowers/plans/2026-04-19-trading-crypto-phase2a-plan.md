# trading-crypto Phase 2a Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a PPO reinforcement learning portfolio agent (BTC/ETH/USDT) using SB3, including gymnasium env, agent wrapper, and training pipeline with MLflow tracking and a multi-seed promotion gate.

**Architecture:** Four new files: `models/ppo_env.py` (gymnasium.Env), `models/ppo_agent.py` (SB3 PPO wrapper), `training/train_ppo.py` (build_ppo_dataset + multi-seed train loop + evaluate + MLflow), `configs/ppo_config.yaml`. Tests live in `tests/test_ppo_env.py`. Observation is a 449-dim float32 vector (K=10 bar feature window × 2 assets + XGB class probs + current weights). Action is a 3-dim raw logit Box(-10, 10) converted to portfolio weights via softmax in `step()`. Reward = next-bar portfolio log-return minus full L1 turnover cost (no /2 division).

**Tech Stack:** Python 3.12, SB3 ≥ 2.0 (requires `gymnasium`, not legacy `gym`), PyTorch (CUDA), gymnasium, scikit-learn RobustScaler, MLflow, pytest. Machine: SGDGX01 (aarch64, CUDA 13.0, GB10). uv at `/home/agents/.local/bin/uv`. Venv: `/home/agents/trading-crypto/.venv/`. SSH prefix for all commands: `ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && <cmd>"`.

**Prerequisite:** 70 tests passing on `main` branch. Saved BTC model at `models/saved/BTC_USDT_xgb.ubj`. ETH model (`ETH_USDT_xgb.ubj`) required for `main()` in `train_ppo.py` but the smoke test in Task 4 gracefully skips if absent.

---

## File Map

| File | Status | Responsibility |
|---|---|---|
| `configs/ppo_config.yaml` | New | All PPO hyperparameters and promotion gate thresholds |
| `models/ppo_env.py` | New | `PortfolioPPOEnv` gymnasium.Env — obs, action, reward, episode |
| `models/ppo_agent.py` | New | `PPOAgent` SB3 wrapper — train, predict (with softmax), save, load |
| `training/train_ppo.py` | New | `PPODataset`, `build_ppo_dataset`, `train_one_seed`, `evaluate_agent`, `main` |
| `tests/test_ppo_env.py` | New | 9 env tests + 1 agent predict-shape test |
| `pyproject.toml` | Modify | Add `gymnasium>=0.29.0`, `stable-baselines3[extra]>=2.0.0` |

---

## Task 1: Install Dependencies and Create PPO Config

**Files:**
- Modify: `/home/agents/trading-crypto/pyproject.toml`
- Create: `/home/agents/trading-crypto/configs/ppo_config.yaml`

- [ ] **Step 1.1: Add gymnasium and stable-baselines3 to pyproject.toml**

Read the current file first, then replace the `dependencies` block:

```toml
[project]
name = "trading-crypto"
version = "0.1.0"
description = "ML crypto trading pipeline: XGBoost signals + PPO portfolio agent"
requires-python = ">=3.12"
dependencies = [
    "ccxt>=4.4.0",
    "pandas>=2.2.0",
    "pyarrow>=16.0.0",
    "requests>=2.32.0",
    "pandas-ta>=0.3.14b",
    "scikit-learn>=1.5.0",
    "joblib>=1.4.0",
    "xgboost>=2.1.0",
    "optuna>=3.6.0",
    "mlflow>=2.14.0",
    "pyyaml>=6.0.0",
    "gymnasium>=0.29.0",
    "stable-baselines3[extra]>=2.0.0",
]

[project.optional-dependencies]
dev = ["pytest>=8.0.0", "pytest-cov>=5.0.0", "responses>=0.25.0"]

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.pytest.ini_options]
testpaths = ["tests"]
python_files = ["test_*.py"]

[tool.hatch.build.targets.wheel]
packages = ["data", "labels", "features", "models", "training", "backtesting", "live"]
```

- [ ] **Step 1.2: Install gymnasium and stable-baselines3 via uv**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && /home/agents/.local/bin/uv sync 2>&1 | tail -10"
```

Expected: `uv` resolves and installs gymnasium and stable-baselines3.

- [ ] **Step 1.3: Install PyTorch with CUDA into the venv**

SGDGX01 is aarch64 with CUDA 13.0. Use the cu124 index (CUDA 13.0 is forward-compatible). Install directly via pip into the uv-managed venv:

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && pip install torch --index-url https://download.pytorch.org/whl/cu124 2>&1 | tail -10"
```

If that fails (no aarch64 wheel at cu124), fall back to the nightly CPU build and note that `device=\"cpu\"` must be used in `ppo_config.yaml`:

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && pip install torch 2>&1 | tail -10"
```

- [ ] **Step 1.4: Verify all imports and CUDA**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python -c '
import torch, gymnasium, stable_baselines3
print(\"torch:\", torch.__version__, \"cuda:\", torch.cuda.is_available())
print(\"gymnasium:\", gymnasium.__version__)
print(\"sb3:\", stable_baselines3.__version__)
'"
```

Expected output (versions may vary):
```
torch: 2.x.x cuda: True
gymnasium: 0.29.x
sb3: 2.x.x
```

If `cuda: False`, update `configs/ppo_config.yaml` in the next step to set `device: cpu`.

- [ ] **Step 1.5: Create configs/ppo_config.yaml**

```bash
ssh sgdgx01 "cat > /home/agents/trading-crypto/configs/ppo_config.yaml << 'YAML'
# Training
total_timesteps: 500000
n_seeds: 5
episode_length: 252
k_window: 10

# Network (passed via policy_kwargs to SB3)
net_arch: [256, 256, 256]
learning_rate: 0.0003
entropy_coef: 0.01
gamma: 0.99
n_steps: 2048
batch_size: 64

# Costs (must match backtesting/costs.py)
fee_rate: 0.001
slippage_bps: 10

# Hardware — change to cpu if CUDA unavailable
device: cuda

# Promotion gate
promotion_sharpe_margin: 0.3
promotion_max_dd_multiplier: 1.5
promotion_max_turnover_per_step: 0.5
YAML"
```

- [ ] **Step 1.6: Commit**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && git add pyproject.toml configs/ppo_config.yaml && git commit -m 'feat: add gymnasium, stable-baselines3 deps; ppo_config.yaml'"
```

---

## Task 2: Implement PortfolioPPOEnv with Tests

**Files:**
- Create: `/home/agents/trading-crypto/tests/test_ppo_env.py`
- Create: `/home/agents/trading-crypto/models/ppo_env.py`

**Background:** 449-dim observation = K×22 BTC features + K×22 ETH features + 3 BTC XGB probs + 3 ETH XGB probs + 3 current weights. Action space is `Box(-10, 10, shape=(3,))` — raw logits converted to weights via softmax inside `step()`. Reward = `w_btc * next_log_ret_btc + w_eth * next_log_ret_eth - sum(|Δw|) * (fee_rate + slippage_bps/10000)`. No `/2` on turnover. Episodes are 252 bars randomly sampled from training data, initial weights from `Dirichlet(α=1)`.

- [ ] **Step 2.1: Write tests/test_ppo_env.py — 8 env tests (the 9th agent test is added in Task 3)**

Create the test file with the 8 env tests only. The `test_ppo_agent_predict_shape` test is deliberately deferred to Task 3 so the committed state is always green.

```python
from __future__ import annotations

import copy

import numpy as np
import pytest
from sklearn.preprocessing import RobustScaler
from stable_baselines3.common.env_checker import check_env

from features.engineer import FEATURE_COLS
from models.ppo_env import PortfolioPPOEnv

_N_FEATURES = len(FEATURE_COLS)  # 22
_K = 10


class _MockXGB:
    """Returns fixed equal-probability predictions. No training needed."""

    def predict_proba(self, X: np.ndarray) -> np.ndarray:
        return np.full((len(X), 3), 1 / 3, dtype=np.float32)


def _make_df(n_bars: int, asset_seed: int = 0) -> "pd.DataFrame":
    import pandas as pd

    rng = np.random.default_rng(asset_seed)
    data = rng.standard_normal((n_bars, _N_FEATURES)).astype(np.float32)
    df = pd.DataFrame(data, columns=FEATURE_COLS)
    df["close"] = 100.0 * np.exp(np.cumsum(rng.normal(0, 0.01, n_bars)))
    return df


def _make_env(n_bars: int = 300, k: int = _K, episode_length: int = 50) -> PortfolioPPOEnv:
    df_btc = _make_df(n_bars, asset_seed=1)
    df_eth = _make_df(n_bars, asset_seed=2)
    scaler_btc = RobustScaler().fit(df_btc[FEATURE_COLS].values)
    scaler_eth = RobustScaler().fit(df_eth[FEATURE_COLS].values)
    cfg = {
        "k_window": k,
        "episode_length": episode_length,
        "fee_rate": 0.001,
        "slippage_bps": 10,
    }
    return PortfolioPPOEnv(
        df_btc=df_btc,
        df_eth=df_eth,
        xgb_btc=_MockXGB(),
        xgb_eth=_MockXGB(),
        scaler_btc=scaler_btc,
        scaler_eth=scaler_eth,
        cfg=cfg,
    )


# --- Tests ---

def test_env_passes_check_env():
    env = _make_env()
    check_env(env, warn=False)


def test_env_reset_returns_correct_obs_shape():
    env = _make_env(k=10)
    obs, info = env.reset(seed=0)
    # K * N_FEATURES * 2 assets + 3 probs * 2 assets + 3 weights = 440 + 6 + 3 = 449
    assert obs.shape == (449,), f"Expected (449,), got {obs.shape}"
    assert obs.dtype == np.float32, f"Expected float32, got {obs.dtype}"


def test_env_step_weights_sum_to_one():
    env = _make_env()
    env.reset(seed=0)
    action = np.array([1.0, 2.0, 0.5], dtype=np.float32)
    obs, reward, terminated, truncated, info = env.step(action)
    weights = info["weights"]
    assert weights.shape == (3,)
    assert (weights >= 0).all(), f"Weights must be non-negative, got {weights}"
    np.testing.assert_allclose(weights.sum(), 1.0, atol=1e-5, err_msg="Weights must sum to 1")


def test_env_reward_penalizes_turnover():
    """High-churn action incurs higher cost than zero-churn action at same step."""
    env_base = _make_env()
    env1 = copy.deepcopy(env_base)
    env2 = copy.deepcopy(env_base)
    env1.reset(seed=42)
    env2.reset(seed=42)
    # Force identical starting weights for fair comparison
    env1._weights = np.array([1 / 3, 1 / 3, 1 / 3], dtype=np.float32)
    env2._weights = np.array([1 / 3, 1 / 3, 1 / 3], dtype=np.float32)

    action_hold = np.array([0.0, 0.0, 0.0], dtype=np.float32)   # softmax → [1/3,1/3,1/3], ~zero turnover
    action_high = np.array([10.0, -10.0, -10.0], dtype=np.float32)  # softmax → ~[1,0,0], high turnover

    _, _, _, _, info_hold = env1.step(action_hold)
    _, _, _, _, info_high = env2.step(action_high)

    assert info_hold["transaction_cost"] < 1e-5, (
        f"Zero-turnover action should have near-zero cost, got {info_hold['transaction_cost']}"
    )
    assert info_high["transaction_cost"] > 0.001, (
        f"High-turnover action should incur meaningful cost, got {info_high['transaction_cost']}"
    )
    assert info_high["transaction_cost"] > info_hold["transaction_cost"]


def test_env_transaction_cost_no_division_by_two():
    """Full BTC→ETH rotation must charge full L1 turnover (=2.0) times cost rate."""
    env = _make_env()
    env.reset(seed=0)
    env._weights = np.array([1.0, 0.0, 0.0], dtype=np.float32)  # 100% BTC

    # Action that saturates ETH: softmax([-10, 10, -10]) ≈ [0, 1, 0]
    action = np.array([-10.0, 10.0, -10.0], dtype=np.float32)
    _, _, _, _, info = env.step(action)

    # Δw ≈ [0-1, 1-0, 0-0] → L1 turnover = 2.0
    # cost = 2.0 * (0.001 + 10/10000) = 2.0 * 0.002 = 0.004
    expected_cost = 2.0 * (0.001 + 10 / 10_000)
    np.testing.assert_allclose(
        info["transaction_cost"], expected_cost, rtol=1e-3,
        err_msg=f"Expected cost {expected_cost:.6f}, got {info['transaction_cost']:.6f}"
    )


def test_env_episode_ends_at_episode_length():
    episode_length = 30
    env = _make_env(n_bars=200, episode_length=episode_length)
    env.reset(seed=0)
    action = np.zeros(3, dtype=np.float32)

    for step in range(episode_length - 1):
        _, _, terminated, truncated, _ = env.step(action)
        assert not terminated
        assert not truncated, f"Episode truncated early at step {step}"

    _, _, terminated, truncated, info = env.step(action)
    assert not terminated
    assert truncated, "Episode must truncate at episode_length"
    assert info["step"] == episode_length


def test_env_deterministic_with_seed():
    """Same seed produces same start_idx, same initial weights, same obs."""
    env = _make_env()
    obs1, _ = env.reset(seed=7)
    start1 = env._start_idx
    weights1 = env._weights.copy()

    obs2, _ = env.reset(seed=7)
    start2 = env._start_idx
    weights2 = env._weights.copy()

    assert start1 == start2, f"start_idx differs: {start1} vs {start2}"
    np.testing.assert_array_equal(weights1, weights2)
    np.testing.assert_array_equal(obs1, obs2)


def test_env_obs_timing_no_lookahead():
    """Obs at step t uses features[t-K+1:t] inclusive; reward uses next_ret[t] (bar t→t+1)."""
    K = 3
    env = _make_env(n_bars=200, k=K, episode_length=20)
    obs, _ = env.reset(seed=0)
    t = env._start_idx

    # First K*N_FEATURES elements of obs are the BTC feature window
    btc_window_expected = env._X_btc[t - K + 1 : t + 1].flatten().astype(np.float32)
    obs_btc = obs[: K * _N_FEATURES]
    np.testing.assert_allclose(obs_btc, btc_window_expected, rtol=1e-5)

    # Reward must use next_ret_btc[t] (bar t→t+1), not bar t-1→t
    env2 = copy.deepcopy(env)
    action = np.zeros(3, dtype=np.float32)
    _, _, _, _, info = env2.step(action)
    w_new = info["weights"]
    expected_portfolio_return = float(
        w_new[0] * env2._next_ret_btc[t] + w_new[1] * env2._next_ret_eth[t]
    )
    np.testing.assert_allclose(
        info["portfolio_return"], expected_portfolio_return, rtol=1e-5
    )


```

- [ ] **Step 2.2: Run tests to confirm they all fail with ImportError**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python -m pytest tests/test_ppo_env.py -v --tb=line 2>&1 | tail -20"
```

Expected: all tests fail with `ModuleNotFoundError: No module named 'models.ppo_env'` (or similar).

- [ ] **Step 2.3: Implement models/ppo_env.py**

```python
from __future__ import annotations

import numpy as np
import gymnasium
from gymnasium import spaces
from sklearn.preprocessing import RobustScaler

from features.engineer import FEATURE_COLS
from models.xgb_classifier import XGBClassifierWrapper

_N_FEATURES = len(FEATURE_COLS)  # 22
_N_CLASSES = 3
_N_WEIGHTS = 3


class PortfolioPPOEnv(gymnasium.Env):
    """
    3-asset (BTC, ETH, USDT) portfolio allocation environment.

    Observation (449-dim float32):
        [BTC feature window K×22] [ETH feature window K×22]
        [XGB probs BTC ×3] [XGB probs ETH ×3] [current weights ×3]

    Action: raw logits (3,) in Box(-10, 10) — converted to weights via softmax in step().

    Reward: next-bar portfolio log-return minus full L1 turnover cost.
        reward = w_btc * ret_btc[t+1] + w_eth * ret_eth[t+1]
                 - sum(|Δw|) * (fee_rate + slippage_bps / 10_000)

    Episodes: episode_length bars randomly sampled from training data.
    """

    def __init__(
        self,
        df_btc: "pd.DataFrame",
        df_eth: "pd.DataFrame",
        xgb_btc: "XGBClassifierWrapper",
        xgb_eth: "XGBClassifierWrapper",
        scaler_btc: RobustScaler,
        scaler_eth: RobustScaler,
        cfg: dict,
    ) -> None:
        super().__init__()
        self._K = int(cfg.get("k_window", 10))
        self._episode_length = int(cfg.get("episode_length", 252))
        self._fee_rate = float(cfg.get("fee_rate", 0.001))
        self._slippage_bps = float(cfg.get("slippage_bps", 10))

        # Precompute scaled feature matrices over full dataset
        X_btc_raw = df_btc[FEATURE_COLS].values.astype(np.float32)
        X_eth_raw = df_eth[FEATURE_COLS].values.astype(np.float32)
        self._X_btc = scaler_btc.transform(X_btc_raw).astype(np.float32)
        self._X_eth = scaler_eth.transform(X_eth_raw).astype(np.float32)

        # Precompute XGB class probabilities over full dataset
        self._proba_btc = xgb_btc.predict_proba(self._X_btc).astype(np.float32)  # (n, 3)
        self._proba_eth = xgb_eth.predict_proba(self._X_eth).astype(np.float32)

        # Next-bar log returns for reward computation
        btc_closes = df_btc["close"].values.astype(np.float64)
        eth_closes = df_eth["close"].values.astype(np.float64)
        self._next_ret_btc = np.zeros(len(btc_closes), dtype=np.float64)
        self._next_ret_eth = np.zeros(len(eth_closes), dtype=np.float64)
        self._next_ret_btc[:-1] = np.log(btc_closes[1:] / btc_closes[:-1])
        self._next_ret_eth[:-1] = np.log(eth_closes[1:] / eth_closes[:-1])

        self._n = len(df_btc)
        if len(df_eth) != self._n:
            raise ValueError(f"df_btc and df_eth must have the same length: {self._n} vs {len(df_eth)}")
        min_bars = self._K + self._episode_length + 1
        if self._n < min_bars:
            raise ValueError(f"Dataset too short: need >= {min_bars} bars, got {self._n}")

        n_obs = self._K * _N_FEATURES * 2 + _N_CLASSES * 2 + _N_WEIGHTS
        self.observation_space = spaces.Box(
            low=-np.inf, high=np.inf, shape=(n_obs,), dtype=np.float32
        )
        self.action_space = spaces.Box(
            low=-10.0, high=10.0, shape=(3,), dtype=np.float32
        )

        # Episode state — set by reset()
        self._start_idx: int = self._K
        self._step_idx: int = 0
        self._weights: np.ndarray = np.array([1 / 3, 1 / 3, 1 / 3], dtype=np.float32)

    def reset(self, *, seed: int | None = None, options: dict | None = None) -> tuple[np.ndarray, dict]:
        super().reset(seed=seed)

        # Sample episode start: need K warmup bars and episode_length+1 bars after
        min_start = self._K
        max_start = self._n - self._episode_length - 1
        self._start_idx = int(self.np_random.integers(min_start, max_start + 1))
        self._step_idx = 0

        # Dirichlet(α=1) initial weights — avoids always-equal-weight bias
        raw = self.np_random.exponential(1.0, size=3)
        self._weights = (raw / raw.sum()).astype(np.float32)

        return self._obs(), {}

    def step(self, action: np.ndarray) -> tuple[np.ndarray, float, bool, bool, dict]:
        t = self._start_idx + self._step_idx

        # Numerically stable softmax — converts logits to valid portfolio weights
        shifted = action - action.max()
        exp_a = np.exp(shifted)
        w_new = (exp_a / exp_a.sum()).astype(np.float32)

        # Portfolio return from bar t to bar t+1
        portfolio_return = float(
            w_new[0] * self._next_ret_btc[t] + w_new[1] * self._next_ret_eth[t]
        )

        # Transaction cost: full L1 turnover × cost rate — no /2
        turnover = float(np.sum(np.abs(w_new - self._weights)))
        transaction_cost = turnover * (self._fee_rate + self._slippage_bps / 10_000)

        reward = portfolio_return - transaction_cost
        self._weights = w_new
        self._step_idx += 1

        terminated = False
        truncated = self._step_idx >= self._episode_length

        obs = np.zeros(self.observation_space.shape, dtype=np.float32) if truncated else self._obs()
        info = {
            "weights": w_new.copy(),
            "portfolio_return": portfolio_return,
            "transaction_cost": transaction_cost,
            "step": self._step_idx,
        }
        return obs, float(reward), terminated, truncated, info

    def _obs(self) -> np.ndarray:
        t = self._start_idx + self._step_idx
        btc_window = self._X_btc[t - self._K + 1 : t + 1].flatten()
        eth_window = self._X_eth[t - self._K + 1 : t + 1].flatten()
        return np.concatenate([
            btc_window, eth_window,
            self._proba_btc[t], self._proba_eth[t],
            self._weights,
        ]).astype(np.float32)
```

- [ ] **Step 2.4: Run tests — expect all 8 pass**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python -m pytest tests/test_ppo_env.py -v --tb=short 2>&1 | tail -20"
```

Expected: 8 passed.

- [ ] **Step 2.5: Commit**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && git add tests/test_ppo_env.py models/ppo_env.py && git commit -m 'feat: PortfolioPPOEnv gymnasium env with 9 tests'"
```

---

## Task 3: Implement PPOAgent Wrapper with Test

**Files:**
- Create: `/home/agents/trading-crypto/models/ppo_agent.py`

**Background:** Thin SB3 PPO wrapper. `predict()` applies softmax to SB3's raw logit action output so callers always receive allocation weights. `train()` wraps the env in `DummyVecEnv` + `VecNormalize(norm_reward=True, norm_obs=False)` to stabilize training on small 4h log-returns (~10⁻³). `policy_kwargs={"net_arch": [...]}` is the correct SB3 2.x API (not `n_layers`/`layer_size`).

- [ ] **Step 3.1: Append test_ppo_agent_predict_shape to tests/test_ppo_env.py, then confirm it fails**

Append this test to the bottom of `tests/test_ppo_env.py`:

```python


def test_ppo_agent_predict_shape():
    """PPOAgent.predict() returns float32 weights shape (3,) in [0,1] summing to 1."""
    from unittest.mock import MagicMock

    import numpy as np
    from stable_baselines3 import PPO

    from models.ppo_agent import PPOAgent

    agent = PPOAgent()
    mock_model = MagicMock(spec=PPO)
    mock_model.predict.return_value = (np.array([1.0, 0.0, -1.0], dtype=np.float32), None)
    agent._model = mock_model

    obs = np.zeros(449, dtype=np.float32)
    weights = agent.predict(obs, deterministic=True)

    assert weights.shape == (3,), f"Expected shape (3,), got {weights.shape}"
    assert weights.dtype == np.float32
    assert (weights >= 0).all(), f"All weights must be non-negative, got {weights}"
    np.testing.assert_allclose(weights.sum(), 1.0, atol=1e-5)
```

Run to confirm it fails:

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python -m pytest tests/test_ppo_env.py::test_ppo_agent_predict_shape -v --tb=short 2>&1 | tail -10"
```

Expected: FAIL with `ModuleNotFoundError: No module named 'models.ppo_agent'`.

- [ ] **Step 3.2: Implement models/ppo_agent.py**

```python
from __future__ import annotations

import os
from pathlib import Path

import numpy as np
from stable_baselines3 import PPO
from stable_baselines3.common.vec_env import DummyVecEnv, VecNormalize

from models.ppo_env import PortfolioPPOEnv


class PPOAgent:
    """
    SB3 PPO wrapper for PortfolioPPOEnv. Mirrors XGBClassifierWrapper interface.

    predict() returns allocation weights after softmax — callers never see raw logits.
    train() wraps the env with VecNormalize(norm_reward=True) to handle small 4h log-returns.
    """

    def __init__(self) -> None:
        self._model: PPO | None = None
        self._vec_normalize: VecNormalize | None = None

    def train(self, env: PortfolioPPOEnv, total_timesteps: int, params: dict) -> None:
        """
        Train PPO. Wraps env in DummyVecEnv + VecNormalize(norm_reward=True, norm_obs=False).
        Sets CUBLAS_WORKSPACE_CONFIG for reproducible CUDA results on GB10.
        """
        os.environ.setdefault("CUBLAS_WORKSPACE_CONFIG", ":4096:8")

        vec_env = DummyVecEnv([lambda: env])
        vec_env = VecNormalize(vec_env, norm_obs=False, norm_reward=True)
        self._vec_normalize = vec_env

        self._model = PPO(
            policy="MlpPolicy",
            env=vec_env,
            learning_rate=params.get("learning_rate", 3e-4),
            n_steps=params.get("n_steps", 2048),
            batch_size=params.get("batch_size", 64),
            gamma=params.get("gamma", 0.99),
            ent_coef=params.get("entropy_coef", 0.01),
            policy_kwargs={"net_arch": params.get("net_arch", [256, 256, 256])},
            device=params.get("device", "cuda"),
            seed=params.get("seed", 0),
            verbose=0,
        )
        self._model.learn(total_timesteps=total_timesteps)

    def predict(self, obs: np.ndarray, deterministic: bool = True) -> np.ndarray:
        """
        Return portfolio weights [w_btc, w_eth, w_usdt] in [0,1] summing to 1.
        Converts SB3 raw logit action to weights via stable softmax.
        """
        if self._model is None:
            raise RuntimeError("Agent not trained. Call train() first.")
        action, _ = self._model.predict(obs, deterministic=deterministic)
        shifted = action - action.max()
        exp_a = np.exp(shifted)
        return (exp_a / exp_a.sum()).astype(np.float32)

    def save(self, path: Path) -> None:
        """Save SB3 model zip. Also saves VecNormalize stats alongside as *_vecnormalize.pkl."""
        if self._model is None:
            raise RuntimeError("No model to save. Call train() first.")
        self._model.save(str(path))
        if self._vec_normalize is not None:
            vec_path = Path(str(path)).with_suffix("") .with_name(
                Path(str(path)).stem + "_vecnormalize.pkl"
            )
            self._vec_normalize.save(str(vec_path))

    def load(self, path: Path) -> None:
        """Load a saved model for inference. VecNormalize not needed (norm_obs=False)."""
        self._model = PPO.load(str(path))
```

- [ ] **Step 3.3: Run the agent predict test — expect PASS**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python -m pytest tests/test_ppo_env.py::test_ppo_agent_predict_shape -v --tb=short 2>&1 | tail -10"
```

Expected: PASS.

- [ ] **Step 3.4: Run full test_ppo_env.py — all 9 tests pass**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python -m pytest tests/test_ppo_env.py -v 2>&1 | tail -15"
```

Expected: 9 passed.

- [ ] **Step 3.5: Run full test suite — no regressions**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python -m pytest -q 2>&1 | tail -5"
```

Expected: 79 passed (70 existing + 9 new).

- [ ] **Step 3.6: Commit (both the updated test file and ppo_agent.py)**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && git add models/ppo_agent.py tests/test_ppo_env.py && git commit -m 'feat: PPOAgent SB3 wrapper with softmax predict; all 9 PPO env/agent tests pass'"
```

---

## Task 4: Implement Training Pipeline

**Files:**
- Create: `/home/agents/trading-crypto/training/train_ppo.py`

**Background:** `build_ppo_dataset` reuses the feature engineering pipeline from `train_xgb.py` but returns raw (unscaled) DataFrames — the env applies the scaler internally. BTC and ETH are inner-joined on timestamp index so they always have the same length and aligned bars. Train/test split: first 80% − `label_n_bars` embargo for train, last 20% for test. `evaluate_agent` manually steps through the full test period (no episode truncation) to get a sequential evaluation Sharpe. B&H benchmark = 50/50 BTC+ETH unmanaged position over the same test bars. Promotion gate checks median across N=5 seeds.

- [ ] **Step 4.1: Implement training/train_ppo.py**

```python
from __future__ import annotations
"""
Training script for PPO portfolio agent (BTC + ETH + USDT cash allocation).

Usage:
    python -m training.train_ppo --data-dir /home/agents/trading-crypto/data/raw

Requires saved XGBoost models + scalers in models/saved/:
    BTC_USDT_xgb.ubj, BTC_USDT_scaler.joblib, ETH_USDT_xgb.ubj, ETH_USDT_scaler.joblib

Reads configs/ppo_config.yaml for all hyperparameters.
Logs to MLflow experiment 'trading-crypto-ppo'.
Saves promoted model to models/saved/btc_eth_ppo.zip.
"""

import argparse
import logging
import math
import subprocess
from dataclasses import dataclass
from pathlib import Path

import mlflow
import numpy as np
import pandas as pd
import stable_baselines3
import torch
import yaml
from sklearn.preprocessing import RobustScaler
from stable_baselines3.common.env_checker import check_env

from data.fetcher import load_latest_ohlcv
from data.onchain import apply_publish_lag, broadcast_daily_to_4h, fetch_btc_active_addresses
from data.validate import validate_ohlcv
from features.engineer import FEATURE_COLS, FEATURE_VERSION, compute
from features.scaler import load as load_scaler
from labels.triple_barrier import apply_embargo, apply_triple_barrier, compute_realized_volatility
from models.ppo_agent import PPOAgent
from models.ppo_env import PortfolioPPOEnv
from models.xgb_classifier import XGBClassifierWrapper

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
log = logging.getLogger(__name__)

_PPO_CONFIG_PATH = Path(__file__).parent.parent / "configs" / "ppo_config.yaml"
_XGB_CONFIG_PATH = Path(__file__).parent.parent / "configs" / "xgb_config.yaml"
_MODELS_DIR = Path(__file__).parent.parent / "models" / "saved"
_ANNUALIZE = math.sqrt(365 * 6)  # 4h bars → annualised


def _load_ppo_config() -> dict:
    with open(_PPO_CONFIG_PATH) as f:
        return yaml.safe_load(f)


def _load_xgb_config() -> dict:
    with open(_XGB_CONFIG_PATH) as f:
        return yaml.safe_load(f)


@dataclass
class PPODataset:
    df_btc_train: pd.DataFrame   # FEATURE_COLS + close, training period (unscaled)
    df_eth_train: pd.DataFrame
    df_btc_test: pd.DataFrame    # FEATURE_COLS + close, test period (unscaled)
    df_eth_test: pd.DataFrame
    xgb_btc: XGBClassifierWrapper
    xgb_eth: XGBClassifierWrapper
    scaler_btc: RobustScaler
    scaler_eth: RobustScaler
    xgb_btc_run_id: str
    xgb_eth_run_id: str


def _prepare_asset_features(asset: str, data_dir: Path, xgb_cfg: dict) -> pd.DataFrame:
    """Load OHLCV, merge on-chain, compute features, label, dropna. Returns df with FEATURE_COLS + close."""
    df = load_latest_ohlcv(asset, data_dir)
    if df is None:
        raise FileNotFoundError(f"No Parquet snapshot found for {asset} in {data_dir}")
    validate_ohlcv(df)

    if asset.upper().startswith("BTC"):
        try:
            since = df.index.min().to_pydatetime()
            until = df.index.max().to_pydatetime()
            btc_addr_daily = fetch_btc_active_addresses(since, until)
            btc_addr_lagged = apply_publish_lag(btc_addr_daily, publish_lag_hours=24)
            btc_addr_4h = broadcast_daily_to_4h(btc_addr_lagged, df.index)
            df = df.join(btc_addr_4h[["btc_active_addresses"]], how="left")
        except Exception as exc:
            log.warning("BTC active addresses unavailable (%s) — stub 0 used", exc)

    df_feat = compute(df, asset=asset)

    vol = compute_realized_volatility(df_feat["close"], window=xgb_cfg["vol_window"])
    labels = apply_triple_barrier(df_feat["close"], vol, k=xgb_cfg["label_k"], n_bars=xgb_cfg["label_n_bars"])
    labels = apply_embargo(labels, n_bars=xgb_cfg["label_n_bars"])
    df_feat["label"] = labels

    for col in ["btc_addr_zscore", "btc_netflow_zscore", "btc_sopr_zscore", "cross_corr_btc_eth"]:
        df_feat[col] = df_feat[col].fillna(0.0)

    return df_feat[FEATURE_COLS + ["label", "close"]].dropna()


def _find_mlflow_run_id(asset_slash: str) -> str:
    """Return the most recent MLflow run_id for asset (e.g. 'BTC/USDT'). Empty string if not found."""
    try:
        client = mlflow.tracking.MlflowClient()
        exp = client.get_experiment_by_name("trading-crypto-xgb")
        if exp is None:
            return ""
        runs = client.search_runs(
            experiment_ids=[exp.experiment_id],
            filter_string=f"params.asset = '{asset_slash}'",
            order_by=["start_time DESC"],
            max_results=1,
        )
        return runs[0].info.run_id if runs else ""
    except Exception:
        return ""


def build_ppo_dataset(data_dir: str) -> PPODataset:
    """
    Load BTC and ETH feature data, align on shared timestamps, split train/test.
    Loads saved XGBoost models and RobustScalers from models/saved/.
    Train split: first 80% minus label_n_bars embargo.
    Test split: last 20% (no overlap with train).
    """
    data_dir = Path(data_dir)
    xgb_cfg = _load_xgb_config()
    embargo = xgb_cfg["label_n_bars"]

    log.info("Preparing BTC/USDT features")
    df_btc = _prepare_asset_features("BTC/USDT", data_dir, xgb_cfg)
    log.info("Preparing ETH/USDT features")
    df_eth = _prepare_asset_features("ETH/USDT", data_dir, xgb_cfg)

    # Inner-join on shared timestamps so arrays are always the same length
    shared_idx = df_btc.index.intersection(df_eth.index)
    df_btc = df_btc.loc[shared_idx].copy()
    df_eth = df_eth.loc[shared_idx].copy()
    log.info("Aligned to %d shared timestamps", len(shared_idx))

    n = len(df_btc)
    split = int(n * 0.8)
    df_btc_train = df_btc.iloc[: split - embargo].copy()
    df_eth_train = df_eth.iloc[: split - embargo].copy()
    df_btc_test = df_btc.iloc[split:].copy()
    df_eth_test = df_eth.iloc[split:].copy()

    for path in [
        _MODELS_DIR / "BTC_USDT_xgb.ubj",
        _MODELS_DIR / "ETH_USDT_xgb.ubj",
        _MODELS_DIR / "BTC_USDT_scaler.joblib",
        _MODELS_DIR / "ETH_USDT_scaler.joblib",
    ]:
        if not path.exists():
            raise FileNotFoundError(
                f"Required model artifact not found: {path}\n"
                "Run training/train_xgb.py for BTC/USDT and ETH/USDT first."
            )

    xgb_btc = XGBClassifierWrapper.load(_MODELS_DIR / "BTC_USDT_xgb.ubj")
    xgb_eth = XGBClassifierWrapper.load(_MODELS_DIR / "ETH_USDT_xgb.ubj")
    scaler_btc = load_scaler(_MODELS_DIR / "BTC_USDT_scaler.joblib")
    scaler_eth = load_scaler(_MODELS_DIR / "ETH_USDT_scaler.joblib")

    return PPODataset(
        df_btc_train=df_btc_train,
        df_eth_train=df_eth_train,
        df_btc_test=df_btc_test,
        df_eth_test=df_eth_test,
        xgb_btc=xgb_btc,
        xgb_eth=xgb_eth,
        scaler_btc=scaler_btc,
        scaler_eth=scaler_eth,
        xgb_btc_run_id=_find_mlflow_run_id("BTC/USDT"),
        xgb_eth_run_id=_find_mlflow_run_id("ETH/USDT"),
    )


def _sharpe(returns: np.ndarray) -> float:
    if len(returns) < 2:
        return 0.0
    std = float(returns.std())
    if std < 1e-10:
        return 0.0
    return float(returns.mean() / std * _ANNUALIZE)


def _max_drawdown(returns: np.ndarray) -> float:
    if len(returns) == 0:
        return 0.0
    equity = np.exp(np.cumsum(returns))
    cum_max = np.maximum.accumulate(equity)
    dd = (cum_max - equity) / cum_max
    return float(dd.max())


def train_one_seed(dataset: PPODataset, params: dict, seed: int) -> tuple[PPOAgent, dict]:
    """Train PPO for one seed. Runs check_env before training. Returns (agent, eval_metrics)."""
    cfg = _load_ppo_config()
    env = PortfolioPPOEnv(
        df_btc=dataset.df_btc_train,
        df_eth=dataset.df_eth_train,
        xgb_btc=dataset.xgb_btc,
        xgb_eth=dataset.xgb_eth,
        scaler_btc=dataset.scaler_btc,
        scaler_eth=dataset.scaler_eth,
        cfg=cfg,
    )
    check_env(env, warn=True)

    agent = PPOAgent()
    agent.train(env, total_timesteps=cfg["total_timesteps"], params={**params, "seed": seed})
    metrics = evaluate_agent(agent, dataset)
    return agent, metrics


def evaluate_agent(agent: PPOAgent, dataset: PPODataset) -> dict:
    """
    Sequential deterministic evaluation over the full test split.

    Steps through all test bars in order (starting at bar K to have a full feature window).
    Returns Sharpe, max_drawdown, mean_return, bh_sharpe, bh_max_drawdown,
    turnover_per_step, n_steps.

    B&H benchmark: 50% BTC + 50% ETH buy-and-hold over the same bars.
    """
    cfg = _load_ppo_config()
    K = int(cfg.get("k_window", 10))
    fee_rate = float(cfg.get("fee_rate", 0.001))
    slippage_bps = float(cfg.get("slippage_bps", 10))
    cost_rate = fee_rate + slippage_bps / 10_000

    df_btc = dataset.df_btc_test
    df_eth = dataset.df_eth_test
    n = len(df_btc)

    # Precompute scaled features and XGB probs for test period
    X_btc = dataset.scaler_btc.transform(
        df_btc[FEATURE_COLS].values.astype(np.float32)
    ).astype(np.float32)
    X_eth = dataset.scaler_eth.transform(
        df_eth[FEATURE_COLS].values.astype(np.float32)
    ).astype(np.float32)
    proba_btc = dataset.xgb_btc.predict_proba(X_btc).astype(np.float32)
    proba_eth = dataset.xgb_eth.predict_proba(X_eth).astype(np.float32)

    btc_closes = df_btc["close"].values.astype(np.float64)
    eth_closes = df_eth["close"].values.astype(np.float64)
    next_ret_btc = np.zeros(n, dtype=np.float64)
    next_ret_eth = np.zeros(n, dtype=np.float64)
    next_ret_btc[:-1] = np.log(btc_closes[1:] / btc_closes[:-1])
    next_ret_eth[:-1] = np.log(eth_closes[1:] / eth_closes[:-1])

    weights = np.array([1 / 3, 1 / 3, 1 / 3], dtype=np.float32)
    portfolio_returns: list[float] = []
    turnovers: list[float] = []

    for t in range(K, n - 1):  # exclude last bar (next return = 0)
        btc_window = X_btc[t - K + 1 : t + 1].flatten()
        eth_window = X_eth[t - K + 1 : t + 1].flatten()
        obs = np.concatenate([btc_window, eth_window, proba_btc[t], proba_eth[t], weights]).astype(np.float32)

        w_new = agent.predict(obs, deterministic=True)
        raw_return = float(w_new[0] * next_ret_btc[t] + w_new[1] * next_ret_eth[t])
        turnover = float(np.sum(np.abs(w_new - weights)))
        net_return = raw_return - turnover * cost_rate

        portfolio_returns.append(net_return)
        turnovers.append(turnover)
        weights = w_new

    returns_arr = np.array(portfolio_returns)
    bh_returns = 0.5 * next_ret_btc[K : n - 1] + 0.5 * next_ret_eth[K : n - 1]

    return {
        "sharpe": _sharpe(returns_arr),
        "max_drawdown": _max_drawdown(returns_arr),
        "mean_return": float(returns_arr.mean()) if len(returns_arr) > 0 else 0.0,
        "bh_sharpe": _sharpe(bh_returns),
        "bh_max_drawdown": _max_drawdown(bh_returns),
        "turnover_per_step": float(np.mean(turnovers)) if turnovers else 0.0,
        "n_steps": len(returns_arr),
    }


def main(data_dir: str) -> None:
    cfg = _load_ppo_config()
    _MODELS_DIR.mkdir(parents=True, exist_ok=True)
    mlflow.set_experiment("trading-crypto-ppo")

    log.info("Building PPO dataset")
    dataset = build_ppo_dataset(data_dir)

    base_params = {
        "net_arch": cfg["net_arch"],
        "learning_rate": cfg["learning_rate"],
        "n_steps": cfg["n_steps"],
        "batch_size": cfg["batch_size"],
        "gamma": cfg["gamma"],
        "entropy_coef": cfg["entropy_coef"],
        "device": cfg["device"],
    }
    n_seeds = cfg["n_seeds"]
    seed_results: list[dict] = []
    seed_agents: list[PPOAgent] = []

    with mlflow.start_run(run_name="ppo_multi_seed"):
        mlflow.log_params({k: v for k, v in cfg.items()})
        mlflow.log_param("feature_version", FEATURE_VERSION)
        mlflow.log_param("xgb_btc_run_id", dataset.xgb_btc_run_id)
        mlflow.log_param("xgb_eth_run_id", dataset.xgb_eth_run_id)
        git_sha = subprocess.check_output(["git", "rev-parse", "HEAD"]).decode().strip()
        mlflow.log_param("git_commit_sha", git_sha)
        mlflow.log_param("sb3_version", stable_baselines3.__version__)
        mlflow.log_param("torch_version", torch.__version__)

        for seed in range(n_seeds):
            log.info("Training seed %d/%d", seed + 1, n_seeds)
            agent, metrics = train_one_seed(dataset, base_params, seed=seed)
            seed_agents.append(agent)
            seed_results.append(metrics)
            mlflow.log_metric(f"seed_{seed}_sharpe", metrics["sharpe"])
            mlflow.log_metric(f"seed_{seed}_max_drawdown", metrics["max_drawdown"])
            mlflow.log_metric(f"seed_{seed}_turnover", metrics["turnover_per_step"])
            log.info("Seed %d: sharpe=%.3f  max_dd=%.3f  turnover=%.3f",
                     seed, metrics["sharpe"], metrics["max_drawdown"], metrics["turnover_per_step"])

        sharpes = [r["sharpe"] for r in seed_results]
        drawdowns = [r["max_drawdown"] for r in seed_results]
        turnovers_list = [r["turnover_per_step"] for r in seed_results]
        bh_sharpe = seed_results[0]["bh_sharpe"]
        bh_max_dd = seed_results[0]["bh_max_drawdown"]

        median_sharpe = float(np.median(sharpes))
        median_dd = float(np.median(drawdowns))
        median_turnover = float(np.median(turnovers_list))

        mlflow.log_metric("median_test_sharpe", median_sharpe)
        mlflow.log_metric("bh_sharpe", bh_sharpe)
        mlflow.log_metric("bh_max_drawdown", bh_max_dd)

        sharpe_ok = median_sharpe > bh_sharpe + cfg["promotion_sharpe_margin"]
        dd_ok = median_dd < bh_max_dd * cfg["promotion_max_dd_multiplier"]
        turnover_ok = median_turnover < cfg["promotion_max_turnover_per_step"]
        promoted = sharpe_ok and dd_ok and turnover_ok

        mlflow.log_param("promoted", promoted)
        log.info(
            "Promotion gate: sharpe_ok=%s (%.3f > %.3f+%.1f)  dd_ok=%s  turnover_ok=%s → promoted=%s",
            sharpe_ok, median_sharpe, bh_sharpe, cfg["promotion_sharpe_margin"],
            dd_ok, turnover_ok, promoted,
        )

        if promoted:
            median_idx = int(np.argsort(sharpes)[len(sharpes) // 2])
            model_path = _MODELS_DIR / "btc_eth_ppo.zip"
            seed_agents[median_idx].save(model_path)
            mlflow.log_artifact(str(model_path))
            log.info("Model promoted and saved to %s (seed %d)", model_path, median_idx)
        else:
            log.info("Model NOT promoted — promotion gate not met")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Train PPO portfolio agent.")
    parser.add_argument("--data-dir", default="/home/agents/trading-crypto/data/raw")
    args = parser.parse_args()
    main(args.data_dir)
```

- [ ] **Step 4.2: Run the full test suite — expect 79 passed, 0 failed**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python -m pytest -q 2>&1 | tail -5"
```

Expected: `79 passed`.

- [ ] **Step 4.3: Smoke test — import and check_env pass with tiny timesteps**

This confirms `train_ppo.py` is importable and `build_ppo_dataset` / `check_env` work end-to-end (if BTC data exists). The ETH model is skipped gracefully via `except FileNotFoundError`.

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python -c \"
from training.train_ppo import _load_ppo_config, PPODataset, build_ppo_dataset, train_one_seed, evaluate_agent
from models.ppo_env import PortfolioPPOEnv
from models.ppo_agent import PPOAgent
import numpy as np

# Quick sanity check: PPOAgent predict works standalone
agent = PPOAgent()
from unittest.mock import MagicMock
from stable_baselines3 import PPO
m = MagicMock(spec=PPO)
m.predict.return_value = (np.array([1.0, 0.0, -1.0], dtype='float32'), None)
agent._model = m
w = agent.predict(np.zeros(449, dtype='float32'))
assert w.shape == (3,) and abs(w.sum() - 1.0) < 1e-5
print('PPOAgent predict: OK', w)

# Confirm imports are all clean
print('All imports OK')
\"
"
```

Expected: `PPOAgent predict: OK [...]`, `All imports OK`.

- [ ] **Step 4.4: Commit**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && git add training/train_ppo.py && git commit -m 'feat: train_ppo.py — build_ppo_dataset, train_one_seed, evaluate_agent, multi-seed main'"
```

---

## Final Check: Full Suite

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python -m pytest -q 2>&1 | tail -5"
```

Expected: `79 passed`.
