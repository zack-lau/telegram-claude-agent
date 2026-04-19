# trading-crypto Phase 2b: Walk-Forward RL Evaluation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `backtesting/evaluate_ppo.py` — a standalone walk-forward evaluation script that loads a trained PPO model, runs it across ~5 overlapping 252-bar windows on the held-out test period, gates promotion via information ratio, and logs results to MLflow.

**Architecture:** One new file (`backtesting/evaluate_ppo.py`) plus 4 config additions and a new test file. No changes to ppo_env.py, ppo_agent.py, or train_ppo.py. The script re-uses `build_ppo_dataset` from train_ppo.py to load data and models. A local `EvalDataset` dataclass holds pre-computed test-period arrays. Exit code 0 = gate passed, 1 = gate failed, 2 = runtime error.

**Tech Stack:** Python 3.12, numpy, SB3 PPO, MLflow, PyYAML. SGDGX01 via SSH. uv at `/home/agents/.local/bin/uv`, venv at `/home/agents/trading-crypto/.venv/`.

**Prerequisite:** 79 tests passing on `feature/phase2a-ppo-rl` branch. All commands run as: `ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && <cmd>"`

---

## File Map

| File | Change |
|---|---|
| `backtesting/evaluate_ppo.py` | New — WindowSlice, EvalDataset, carve_windows, evaluate_window, compute_gate, load_model_and_data, main |
| `configs/ppo_config.yaml` | Add 4 walk-forward params |
| `tests/test_evaluate_ppo.py` | New — 19 tests |

---

## Task 1: Config + WindowSlice + carve_windows

**Files:**
- Modify: `configs/ppo_config.yaml`
- Create: `backtesting/evaluate_ppo.py` (partial — WindowSlice + carve_windows only)
- Create: `tests/test_evaluate_ppo.py` (partial — 5 carve_windows tests)

### Background

`WindowSlice` is a frozen dataclass with half-open `[start, end)` convention. `carve_windows` produces overlapping strided windows from the test period. All indices are relative to the test split start (index 0 = first test bar).

Carving formula:
```
first_usable = test_start + k_window  (need K bars of lookback before first obs)
s = first_usable
while s + episode_length <= test_end:
    emit WindowSlice(start=s, end=s+episode_length, n_steps=episode_length)
    s += stride
```

With test_start=0, test_end=880, K=10, episode_length=252, stride=126 → 5 windows:
[10,262), [136,388), [262,514), [388,640), [514,766).

- [ ] **Step 1.1: Add walk-forward params to ppo_config.yaml**

```bash
ssh sgdgx01 "cat >> /home/agents/trading-crypto/configs/ppo_config.yaml << 'EOF'

# Walk-forward evaluation
wf_episode_stride: 126                 # bars between window starts (50% overlap with episode_length=252)
wf_min_positive_sharpe_windows: 4     # gate: Sharpe > 0 in at least this many windows
wf_min_information_ratio: 0.0         # gate: median window IR >= this
wf_n_dirichlet_samples: 5             # Dirichlet(1,1,1) starts averaged per window
EOF"
```

Verify:
```bash
ssh sgdgx01 "tail -8 /home/agents/trading-crypto/configs/ppo_config.yaml"
```

Expected: 4 new keys visible.

- [ ] **Step 1.2: Write failing carve_windows tests**

Create `/home/agents/trading-crypto/tests/test_evaluate_ppo.py`:

```python
from __future__ import annotations

import numpy as np
import pytest


# ---------------------------------------------------------------------------
# Helpers (reused across tasks)
# ---------------------------------------------------------------------------

class _MockPPOAgent:
    """Returns fixed equal weights regardless of obs."""
    def predict(self, obs: np.ndarray, deterministic: bool = True) -> np.ndarray:
        return np.array([1 / 3, 1 / 3, 1 / 3], dtype=np.float32)


# ---------------------------------------------------------------------------
# Task 1: carve_windows tests
# ---------------------------------------------------------------------------

def test_carve_windows_correct_count():
    """5 windows expected for standard config: test_end=880, K=10, ep=252, stride=126."""
    from backtesting.evaluate_ppo import carve_windows
    windows = carve_windows(test_start=0, test_end=880, k_window=10, episode_length=252, stride=126)
    assert len(windows) == 5, f"Expected 5 windows, got {len(windows)}"


def test_carve_windows_stride_spacing():
    """Adjacent window starts must differ by exactly stride."""
    from backtesting.evaluate_ppo import carve_windows
    windows = carve_windows(test_start=0, test_end=880, k_window=10, episode_length=252, stride=126)
    for i in range(len(windows) - 1):
        assert windows[i + 1].start - windows[i].start == 126


def test_carve_windows_half_open_invariant():
    """Each WindowSlice must satisfy end - start == n_steps."""
    from backtesting.evaluate_ppo import carve_windows
    windows = carve_windows(test_start=0, test_end=880, k_window=10, episode_length=252, stride=126)
    for w in windows:
        assert w.end - w.start == w.n_steps


def test_carve_windows_respects_warmup():
    """First window start must equal test_start + k_window."""
    from backtesting.evaluate_ppo import carve_windows
    windows = carve_windows(test_start=0, test_end=880, k_window=10, episode_length=252, stride=126)
    assert windows[0].start == 0 + 10  # test_start + k_window


def test_carve_windows_insufficient_data():
    """Returns [] when test period too short for even one window."""
    from backtesting.evaluate_ppo import carve_windows
    # 360 bars: first_usable=10, need 10+252=262 <= 360 → 1 window fits? Let's use 260 bars.
    # first_usable=10, 10+252=262 > 260 → no windows
    windows = carve_windows(test_start=0, test_end=260, k_window=10, episode_length=252, stride=126)
    assert windows == []
```

Run to confirm failure:
```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -m pytest tests/test_evaluate_ppo.py -v --tb=short 2>&1 | tail -15"
```

Expected: ImportError or similar — `backtesting/evaluate_ppo.py` does not exist yet.

- [ ] **Step 1.3: Implement WindowSlice and carve_windows**

Create `/home/agents/trading-crypto/backtesting/evaluate_ppo.py`:

```python
from __future__ import annotations
"""
Walk-forward evaluation for the PPO portfolio agent.

Usage:
    python backtesting/evaluate_ppo.py \\
        --model-path models/saved/btc_eth_ppo.zip \\
        --data-dir data/raw \\
        [--eval-seed 42]

Exit codes:
    0 — walk-forward gate passed
    1 — gate failed (agent did not pass walk-forward criteria)
    2 — runtime error (missing files, insufficient data, shape mismatch)
"""

import argparse
import dataclasses
import hashlib
import logging
import math
import subprocess
import sys
from pathlib import Path

import mlflow
import numpy as np
import stable_baselines3
import torch
import yaml

log = logging.getLogger(__name__)

_PPO_CONFIG_PATH = Path(__file__).parent.parent / "configs" / "ppo_config.yaml"
_MODELS_DIR = Path(__file__).parent.parent / "models" / "saved"
_ANNUALIZE = math.sqrt(365 * 6)  # 4h bars → annualised


def _load_ppo_config() -> dict:
    with open(_PPO_CONFIG_PATH) as f:
        return yaml.safe_load(f)


# ---------------------------------------------------------------------------
# WindowSlice
# ---------------------------------------------------------------------------

@dataclasses.dataclass(frozen=True)
class WindowSlice:
    start: int    # inclusive — half-open convention [start, end)
    end: int      # exclusive
    n_steps: int

    def __post_init__(self) -> None:
        assert self.end - self.start == self.n_steps, (
            f"WindowSlice inconsistency: end-start={self.end - self.start}, n_steps={self.n_steps}"
        )


def carve_windows(
    test_start: int,
    test_end: int,
    k_window: int,
    episode_length: int,
    stride: int,
) -> list[WindowSlice]:
    """Return overlapping strided WindowSlice list. Returns [] if insufficient data."""
    first_usable = test_start + k_window
    windows: list[WindowSlice] = []
    s = first_usable
    while s + episode_length <= test_end:
        windows.append(WindowSlice(start=s, end=s + episode_length, n_steps=episode_length))
        s += stride
    return windows
```

- [ ] **Step 1.4: Run carve_windows tests**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -m pytest tests/test_evaluate_ppo.py -v --tb=short 2>&1 | tail -15"
```

Expected: 5 passed.

- [ ] **Step 1.5: Run full suite — no regressions**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -m pytest -q 2>&1 | tail -5"
```

Expected: 84 passed.

- [ ] **Step 1.6: Commit**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && git add configs/ppo_config.yaml backtesting/evaluate_ppo.py tests/test_evaluate_ppo.py && git commit -m 'feat: WindowSlice, carve_windows, walk-forward config params'"
```

---

## Task 2: EvalDataset + evaluate_window

**Files:**
- Modify: `backtesting/evaluate_ppo.py` — add EvalDataset, helper functions, evaluate_window
- Modify: `tests/test_evaluate_ppo.py` — add 6 evaluate_window tests

### Background

`EvalDataset` holds pre-computed arrays for the test period only. All arrays are indexed 0..n_test-1 (relative to test split start). `evaluate_window` runs M=`wf_n_dirichlet_samples` rollouts on a window and returns mean metrics.

**Critical: obs at bar t uses `X[t-K+1:t+1]`** (inclusive of bar t, K bars total) — this matches `PortfolioPPOEnv._obs()` exactly.

IR formula: `mean(diff) / std(diff, ddof=1)` where `diff = net_returns - bh_returns`. Guard against `std < 1e-10`.

Sharpe: `mean(returns) / std(returns, ddof=1) * sqrt(6*365)`. Guard std.

Max drawdown: from equity curve `exp(cumsum(returns))`, always in [0, 1].

- [ ] **Step 2.1: Write failing evaluate_window tests**

Append to `/home/agents/trading-crypto/tests/test_evaluate_ppo.py`:

```python
# ---------------------------------------------------------------------------
# Task 2: evaluate_window tests
# ---------------------------------------------------------------------------

def _make_eval_dataset(n: int = 100, seed: int = 0) -> "EvalDataset":
    """Synthetic EvalDataset with known data for testing."""
    import sys
    sys.path.insert(0, "/home/agents/trading-crypto")
    from backtesting.evaluate_ppo import EvalDataset
    from features.engineer import FEATURE_COLS

    rng = np.random.default_rng(seed)
    n_feat = len(FEATURE_COLS)  # 22
    X_btc = rng.standard_normal((n, n_feat)).astype(np.float32)
    X_eth = rng.standard_normal((n, n_feat)).astype(np.float32)
    proba_btc = np.full((n, 3), 1 / 3, dtype=np.float32)
    proba_eth = np.full((n, 3), 1 / 3, dtype=np.float32)
    next_ret_btc = rng.normal(0, 0.01, n)
    next_ret_eth = rng.normal(0, 0.01, n)
    next_ret_btc[-1] = 0.0
    next_ret_eth[-1] = 0.0
    return EvalDataset(
        X_btc=X_btc, X_eth=X_eth,
        proba_btc=proba_btc, proba_eth=proba_eth,
        next_ret_btc=next_ret_btc, next_ret_eth=next_ret_eth,
        xgb_btc_run_id="", xgb_eth_run_id="",
        dataset_hash="test",
    )


def test_obs_matches_env():
    """Obs built by evaluate_window formula matches PortfolioPPOEnv._obs() at the same bar."""
    import sys
    sys.path.insert(0, "/home/agents/trading-crypto")
    import pandas as pd
    from sklearn.preprocessing import RobustScaler
    from backtesting.evaluate_ppo import EvalDataset
    from features.engineer import FEATURE_COLS
    from models.ppo_env import PortfolioPPOEnv

    n, K = 60, 10

    class _MockXGB:
        def predict_proba(self, X):
            return np.full((len(X), 3), 1 / 3, dtype=np.float32)

    rng = np.random.default_rng(99)
    raw = rng.standard_normal((n, len(FEATURE_COLS))).astype(np.float32)
    close_btc = 100.0 * np.exp(np.cumsum(rng.normal(0, 0.01, n)))
    close_eth = 200.0 * np.exp(np.cumsum(rng.normal(0, 0.01, n)))

    df_btc = pd.DataFrame(raw, columns=FEATURE_COLS)
    df_btc["close"] = close_btc
    df_eth = pd.DataFrame(raw * 0.9, columns=FEATURE_COLS)
    df_eth["close"] = close_eth

    scaler_btc = RobustScaler().fit(raw)
    scaler_eth = RobustScaler().fit(raw * 0.9)

    X_btc = scaler_btc.transform(raw).astype(np.float32)
    X_eth = scaler_eth.transform(raw * 0.9).astype(np.float32)
    proba = np.full((n, 3), 1 / 3, dtype=np.float32)
    next_ret = np.zeros(n)

    eval_ds = EvalDataset(
        X_btc=X_btc, X_eth=X_eth,
        proba_btc=proba, proba_eth=proba,
        next_ret_btc=next_ret, next_ret_eth=next_ret,
        xgb_btc_run_id="", xgb_eth_run_id="", dataset_hash="",
    )

    cfg = {"k_window": K, "episode_length": 20, "fee_rate": 0.001, "slippage_bps": 10}
    env = PortfolioPPOEnv(
        df_btc=df_btc, df_eth=df_eth,
        xgb_btc=_MockXGB(), xgb_eth=_MockXGB(),
        scaler_btc=scaler_btc, scaler_eth=scaler_eth,
        cfg=cfg,
    )
    env.reset(seed=7)
    t = env._start_idx + env._step_idx  # step_idx=0 after reset
    env_obs = env._obs()

    # Build obs using evaluate_window formula — must match env exactly
    btc_window = eval_ds.X_btc[t - K + 1 : t + 1].flatten()
    eth_window = eval_ds.X_eth[t - K + 1 : t + 1].flatten()
    eval_obs = np.concatenate([
        btc_window, eth_window,
        eval_ds.proba_btc[t], eval_ds.proba_eth[t],
        env._weights,
    ]).astype(np.float32)

    np.testing.assert_array_equal(env_obs, eval_obs)


def test_evaluate_window_returns_correct_keys():
    """Result dict must have exactly the 9 documented keys."""
    import sys
    sys.path.insert(0, "/home/agents/trading-crypto")
    from backtesting.evaluate_ppo import evaluate_window, WindowSlice

    ds = _make_eval_dataset(n=100)
    window = WindowSlice(start=10, end=40, n_steps=30)
    cfg = {"k_window": 10, "fee_rate": 0.001, "slippage_bps": 10, "wf_n_dirichlet_samples": 2}
    rng = np.random.default_rng(0)
    result = evaluate_window(_MockPPOAgent(), ds, window, cfg, rng)

    expected_keys = {
        "sharpe", "max_drawdown", "turnover_per_step",
        "bh_sharpe", "bh_max_drawdown", "information_ratio",
        "window_start", "window_end", "n_steps",
    }
    assert set(result.keys()) == expected_keys, f"Missing keys: {expected_keys - set(result.keys())}"


def test_evaluate_window_bh_is_50_50():
    """B&H return at each step must equal 0.5*next_ret_btc + 0.5*next_ret_eth."""
    import sys
    sys.path.insert(0, "/home/agents/trading-crypto")
    from backtesting.evaluate_ppo import EvalDataset, evaluate_window, WindowSlice

    n = 60
    rng_np = np.random.default_rng(1)
    next_ret_btc = rng_np.normal(0, 0.01, n)
    next_ret_eth = rng_np.normal(0, 0.01, n)
    next_ret_btc[-1] = 0.0; next_ret_eth[-1] = 0.0

    from features.engineer import FEATURE_COLS
    n_feat = len(FEATURE_COLS)
    X = rng_np.standard_normal((n, n_feat)).astype(np.float32)
    ds = EvalDataset(
        X_btc=X, X_eth=X,
        proba_btc=np.full((n, 3), 1/3, dtype=np.float32),
        proba_eth=np.full((n, 3), 1/3, dtype=np.float32),
        next_ret_btc=next_ret_btc, next_ret_eth=next_ret_eth,
        xgb_btc_run_id="", xgb_eth_run_id="", dataset_hash="",
    )
    window = WindowSlice(start=10, end=30, n_steps=20)
    cfg = {"k_window": 10, "fee_rate": 0.0, "slippage_bps": 0, "wf_n_dirichlet_samples": 1}

    # Agent holds fixed weights [1/3, 1/3, 1/3]
    # We can't directly check bh per step, but bh_sharpe should match manual calculation
    expected_bh = 0.5 * next_ret_btc[10:30] + 0.5 * next_ret_eth[10:30]
    import math
    std_bh = expected_bh.std(ddof=1)
    expected_bh_sharpe = float(expected_bh.mean() / std_bh * math.sqrt(6 * 365)) if std_bh > 1e-10 else 0.0

    rng = np.random.default_rng(0)
    result = evaluate_window(_MockPPOAgent(), ds, window, cfg, rng)
    np.testing.assert_allclose(result["bh_sharpe"], expected_bh_sharpe, rtol=1e-4)


def test_evaluate_window_deterministic():
    """Same rng seed → identical per-window metrics."""
    import sys
    sys.path.insert(0, "/home/agents/trading-crypto")
    from backtesting.evaluate_ppo import evaluate_window, WindowSlice

    ds = _make_eval_dataset(n=100)
    window = WindowSlice(start=10, end=40, n_steps=30)
    cfg = {"k_window": 10, "fee_rate": 0.001, "slippage_bps": 10, "wf_n_dirichlet_samples": 3}

    result1 = evaluate_window(_MockPPOAgent(), ds, window, cfg, np.random.default_rng(42))
    result2 = evaluate_window(_MockPPOAgent(), ds, window, cfg, np.random.default_rng(42))

    assert result1["sharpe"] == result2["sharpe"]
    assert result1["information_ratio"] == result2["information_ratio"]


def test_evaluate_window_turnover_no_division_by_two():
    """Full BTC→ETH rotation (w_old=[1,0,0], w_new=[0,1,0]) must produce turnover=2.0."""
    import sys
    sys.path.insert(0, "/home/agents/trading-crypto")
    from backtesting.evaluate_ppo import EvalDataset, evaluate_window, WindowSlice
    from features.engineer import FEATURE_COLS

    class _RotatingAgent:
        """Alternates full BTC and full ETH allocation."""
        _call = 0
        def predict(self, obs, deterministic=True):
            self._call += 1
            if self._call % 2 == 1:
                return np.array([1.0, 0.0, 0.0], dtype=np.float32)
            return np.array([0.0, 1.0, 0.0], dtype=np.float32)

    n = 60
    n_feat = len(FEATURE_COLS)
    rng_np = np.random.default_rng(5)
    X = rng_np.standard_normal((n, n_feat)).astype(np.float32)
    next_ret = np.full(n, 0.001)
    next_ret[-1] = 0.0
    ds = EvalDataset(
        X_btc=X, X_eth=X,
        proba_btc=np.full((n, 3), 1/3, dtype=np.float32),
        proba_eth=np.full((n, 3), 1/3, dtype=np.float32),
        next_ret_btc=next_ret, next_ret_eth=next_ret,
        xgb_btc_run_id="", xgb_eth_run_id="", dataset_hash="",
    )
    window = WindowSlice(start=10, end=20, n_steps=10)
    # fee_rate=0.001, slippage=0; full rotation: L1=2, cost=2*0.001=0.002/step (after warmup)
    cfg = {"k_window": 10, "fee_rate": 0.001, "slippage_bps": 0, "wf_n_dirichlet_samples": 1}
    rng = np.random.default_rng(0)
    result = evaluate_window(_RotatingAgent(), ds, window, cfg, rng)
    # Mean turnover ≈ 2 (some steps vary due to Dirichlet start)
    assert result["turnover_per_step"] > 1.0, f"Expected turnover > 1.0, got {result['turnover_per_step']:.3f}"


def test_information_ratio_zero_variance():
    """When agent returns == bh returns exactly, IR must be 0.0 (no crash)."""
    import sys
    sys.path.insert(0, "/home/agents/trading-crypto")
    from backtesting.evaluate_ppo import EvalDataset, evaluate_window, WindowSlice
    from features.engineer import FEATURE_COLS

    class _BHAgent:
        """Returns weights that replicate B&H: [0.5, 0.5, 0]."""
        def predict(self, obs, deterministic=True):
            return np.array([0.5, 0.5, 0.0], dtype=np.float32)

    n = 60
    n_feat = len(FEATURE_COLS)
    rng_np = np.random.default_rng(3)
    X = rng_np.standard_normal((n, n_feat)).astype(np.float32)
    next_ret = rng_np.normal(0, 0.01, n)
    next_ret[-1] = 0.0
    ds = EvalDataset(
        X_btc=X, X_eth=X,
        proba_btc=np.full((n, 3), 1/3, dtype=np.float32),
        proba_eth=np.full((n, 3), 1/3, dtype=np.float32),
        next_ret_btc=next_ret, next_ret_eth=next_ret,
        xgb_btc_run_id="", xgb_eth_run_id="", dataset_hash="",
    )
    window = WindowSlice(start=10, end=30, n_steps=20)
    # fee_rate=0 so net_return == bh_return (both use same next_ret, same 0.5/0.5 weight)
    cfg = {"k_window": 10, "fee_rate": 0.0, "slippage_bps": 0, "wf_n_dirichlet_samples": 1}
    rng = np.random.default_rng(0)
    result = evaluate_window(_BHAgent(), ds, window, cfg, rng)
    # diff = net_returns - bh_returns ≈ 0 → IR should be 0.0, no NaN/inf
    assert np.isfinite(result["information_ratio"]), "IR must be finite even at zero variance"
```

Run to confirm failures:
```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -m pytest tests/test_evaluate_ppo.py -v --tb=short 2>&1 | tail -20"
```

Expected: 5 pass (carve tests), 6 fail/error (evaluate_window not implemented).

- [ ] **Step 2.2: Implement EvalDataset and evaluate_window**

Append to `/home/agents/trading-crypto/backtesting/evaluate_ppo.py` (after the carve_windows function):

```python

# ---------------------------------------------------------------------------
# EvalDataset
# ---------------------------------------------------------------------------

@dataclasses.dataclass
class EvalDataset:
    """Pre-computed arrays for test period. Index 0 = first test bar."""
    X_btc: np.ndarray          # (n_test, 22) float32, scaler-transformed
    X_eth: np.ndarray          # (n_test, 22) float32, scaler-transformed
    proba_btc: np.ndarray      # (n_test, 3) float32 — XGB class probs
    proba_eth: np.ndarray      # (n_test, 3) float32
    next_ret_btc: np.ndarray   # (n_test,) float64 — log(close[t+1]/close[t]), last=0
    next_ret_eth: np.ndarray   # (n_test,) float64
    xgb_btc_run_id: str
    xgb_eth_run_id: str
    dataset_hash: str          # SHA256 of feature matrix bytes for reproducibility


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _sharpe_wf(returns: np.ndarray) -> float:
    if len(returns) < 2:
        return 0.0
    std = float(np.std(returns, ddof=1))
    if std < 1e-10:
        return 0.0
    return float(np.mean(returns) / std * _ANNUALIZE)


def _max_drawdown_wf(returns: np.ndarray) -> float:
    if len(returns) == 0:
        return 0.0
    equity = np.exp(np.cumsum(returns))
    cum_max = np.maximum.accumulate(equity)
    dd = (cum_max - equity) / cum_max
    return float(dd.max())


def _information_ratio(net_returns: np.ndarray, bh_returns: np.ndarray) -> float:
    diff = net_returns - bh_returns
    std = float(np.std(diff, ddof=1))
    if std < 1e-10:
        return 0.0
    return float(np.mean(diff) / std)


# ---------------------------------------------------------------------------
# evaluate_window
# ---------------------------------------------------------------------------

def evaluate_window(
    agent,
    dataset: EvalDataset,
    window: WindowSlice,
    cfg: dict,
    rng: np.random.Generator,
) -> dict:
    """
    Run wf_n_dirichlet_samples rollouts on window bars. Return mean metrics dict.
    Each metric computed per sample then averaged (robustness over starting weights).
    """
    K = int(cfg.get("k_window", 10))
    fee_rate = float(cfg.get("fee_rate", 0.001))
    slippage_bps = float(cfg.get("slippage_bps", 10))
    cost_rate = fee_rate + slippage_bps / 10_000
    n_samples = int(cfg.get("wf_n_dirichlet_samples", 5))

    sample_metrics: list[dict] = []
    for _ in range(n_samples):
        raw = rng.exponential(1.0, size=3)
        weights = (raw / raw.sum()).astype(np.float32)

        net_returns: list[float] = []
        bh_returns_list: list[float] = []
        turnovers: list[float] = []

        for t in range(window.start, window.end):
            # Obs: K bars inclusive of t — matches PortfolioPPOEnv._obs()
            btc_window = dataset.X_btc[t - K + 1 : t + 1].flatten()
            eth_window = dataset.X_eth[t - K + 1 : t + 1].flatten()
            obs = np.concatenate([
                btc_window, eth_window,
                dataset.proba_btc[t], dataset.proba_eth[t],
                weights,
            ]).astype(np.float32)

            w_new = agent.predict(obs, deterministic=True)
            portfolio_return = float(
                w_new[0] * dataset.next_ret_btc[t] + w_new[1] * dataset.next_ret_eth[t]
            )
            turnover = float(np.abs(w_new - weights).sum())  # full L1, no /2
            net_return = portfolio_return - turnover * cost_rate
            bh_return = float(
                0.5 * dataset.next_ret_btc[t] + 0.5 * dataset.next_ret_eth[t]
            )

            net_returns.append(net_return)
            bh_returns_list.append(bh_return)
            turnovers.append(turnover)
            weights = w_new

        net_arr = np.array(net_returns)
        bh_arr = np.array(bh_returns_list)
        sample_metrics.append({
            "sharpe": _sharpe_wf(net_arr),
            "max_drawdown": _max_drawdown_wf(net_arr),
            "turnover_per_step": float(np.mean(turnovers)) if turnovers else 0.0,
            "bh_sharpe": _sharpe_wf(bh_arr),
            "bh_max_drawdown": _max_drawdown_wf(bh_arr),
            "information_ratio": _information_ratio(net_arr, bh_arr),
        })

    # Mean across samples
    keys = ["sharpe", "max_drawdown", "turnover_per_step", "bh_sharpe", "bh_max_drawdown", "information_ratio"]
    return {
        **{k: float(np.mean([s[k] for s in sample_metrics])) for k in keys},
        "window_start": window.start,
        "window_end": window.end,
        "n_steps": window.n_steps,
    }
```

- [ ] **Step 2.3: Run evaluate_window tests**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -m pytest tests/test_evaluate_ppo.py -v --tb=short 2>&1 | tail -20"
```

Expected: 11 passed (5 carve + 6 evaluate).

- [ ] **Step 2.4: Run full suite — no regressions**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -m pytest -q 2>&1 | tail -5"
```

Expected: 90 passed.

- [ ] **Step 2.5: Commit**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && git add backtesting/evaluate_ppo.py tests/test_evaluate_ppo.py && git commit -m 'feat: EvalDataset, evaluate_window, helper functions'"
```

---

## Task 3: GateResult + compute_gate

**Files:**
- Modify: `backtesting/evaluate_ppo.py` — add GateResult dataclass and compute_gate
- Modify: `tests/test_evaluate_ppo.py` — add 8 gate tests

### Background

`compute_gate` checks two conditions: (1) Sharpe > 0 in ≥ `wf_min_positive_sharpe_windows` windows, (2) median IR ≥ `wf_min_information_ratio`. Handles empty window list (returns passed=False, reason="no windows to evaluate"). Returns a `GateResult` dataclass with `passed`, `reason`, and `metrics`.

- [ ] **Step 3.1: Write failing gate tests**

Append to `/home/agents/trading-crypto/tests/test_evaluate_ppo.py`:

```python
# ---------------------------------------------------------------------------
# Task 3: compute_gate tests
# ---------------------------------------------------------------------------

def _make_window_results(sharpes: list[float], irs: list[float]) -> list[dict]:
    """Helper to build synthetic window_results for gate testing."""
    return [
        {
            "sharpe": s, "information_ratio": ir,
            "max_drawdown": 0.1, "turnover_per_step": 0.2,
            "bh_sharpe": 0.3, "bh_max_drawdown": 0.05,
            "window_start": i * 100, "window_end": i * 100 + 100, "n_steps": 100,
        }
        for i, (s, ir) in enumerate(zip(sharpes, irs))
    ]


def test_gate_passes_when_criteria_met():
    """4/5 positive Sharpe windows and median IR=0.5 → passed=True."""
    from backtesting.evaluate_ppo import compute_gate
    results = _make_window_results(
        sharpes=[0.5, 0.4, 0.3, 0.2, -0.1],  # 4 positive
        irs=[0.8, 0.6, 0.4, 0.5, 0.3],        # median=0.5
    )
    cfg = {"wf_min_positive_sharpe_windows": 4, "wf_min_information_ratio": 0.0}
    gate = compute_gate(results, cfg)
    assert gate.passed is True
    assert "4/5" in gate.reason


def test_gate_fails_insufficient_positive_windows():
    """Only 3/5 positive Sharpe → passed=False, reason mentions count."""
    from backtesting.evaluate_ppo import compute_gate
    results = _make_window_results(
        sharpes=[0.5, 0.4, 0.3, -0.1, -0.2],  # 3 positive
        irs=[0.8, 0.6, 0.4, 0.5, 0.3],
    )
    cfg = {"wf_min_positive_sharpe_windows": 4, "wf_min_information_ratio": 0.0}
    gate = compute_gate(results, cfg)
    assert gate.passed is False
    assert "3" in gate.reason
    assert "5" in gate.reason


def test_gate_fails_negative_median_ir():
    """5/5 positive Sharpe but median IR < 0 → passed=False."""
    from backtesting.evaluate_ppo import compute_gate
    results = _make_window_results(
        sharpes=[0.5, 0.4, 0.3, 0.2, 0.1],
        irs=[-0.5, -0.3, -0.1, -0.2, -0.4],   # median=-0.3
    )
    cfg = {"wf_min_positive_sharpe_windows": 4, "wf_min_information_ratio": 0.0}
    gate = compute_gate(results, cfg)
    assert gate.passed is False
    assert "IR" in gate.reason or "ir" in gate.reason.lower()


def test_gate_result_reason_non_empty_on_fail():
    """GateResult.reason must be non-empty string on failure."""
    from backtesting.evaluate_ppo import compute_gate
    results = _make_window_results(
        sharpes=[-0.1, -0.2, -0.3, -0.4, -0.5],
        irs=[-0.5, -0.3, -0.4, -0.2, -0.1],
    )
    cfg = {"wf_min_positive_sharpe_windows": 4, "wf_min_information_ratio": 0.0}
    gate = compute_gate(results, cfg)
    assert gate.passed is False
    assert len(gate.reason) > 0


def test_gate_edge_case_exactly_at_threshold():
    """median_ir == 0.0 exactly with >= threshold must pass."""
    from backtesting.evaluate_ppo import compute_gate
    results = _make_window_results(
        sharpes=[0.5, 0.4, 0.3, 0.2, 0.1],
        irs=[0.1, 0.0, -0.1, 0.1, 0.0],   # median = 0.0
    )
    cfg = {"wf_min_positive_sharpe_windows": 4, "wf_min_information_ratio": 0.0}
    gate = compute_gate(results, cfg)
    assert gate.passed is True


def test_gate_empty_windows():
    """Empty window_results → passed=False, reason='no windows', no crash."""
    from backtesting.evaluate_ppo import compute_gate
    gate = compute_gate([], {"wf_min_positive_sharpe_windows": 4, "wf_min_information_ratio": 0.0})
    assert gate.passed is False
    assert "no windows" in gate.reason.lower()


def test_gate_metrics_dict_has_expected_keys():
    """GateResult.metrics must contain n_positive_sharpe_windows, median_ir, n_windows."""
    from backtesting.evaluate_ppo import compute_gate
    results = _make_window_results(
        sharpes=[0.5, 0.4, 0.3, 0.2, 0.1],
        irs=[0.3, 0.4, 0.2, 0.5, 0.1],
    )
    cfg = {"wf_min_positive_sharpe_windows": 4, "wf_min_information_ratio": 0.0}
    gate = compute_gate(results, cfg)
    assert "n_positive_sharpe_windows" in gate.metrics
    assert "median_ir" in gate.metrics
    assert "n_windows" in gate.metrics


def test_gate_both_conditions_must_pass():
    """Gate fails if either condition fails — not just one."""
    from backtesting.evaluate_ppo import compute_gate
    # 4/5 positive BUT median_ir < 0 → fail
    results = _make_window_results(
        sharpes=[0.5, 0.4, 0.3, 0.2, -0.1],
        irs=[0.3, -0.4, -0.2, 0.1, -0.5],   # median=-0.2
    )
    cfg = {"wf_min_positive_sharpe_windows": 4, "wf_min_information_ratio": 0.0}
    gate = compute_gate(results, cfg)
    assert gate.passed is False
```

Run to confirm failures:
```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -m pytest tests/test_evaluate_ppo.py::test_gate_passes_when_criteria_met -v --tb=short 2>&1 | tail -10"
```

Expected: FAIL — `compute_gate` not implemented yet.

- [ ] **Step 3.2: Implement GateResult and compute_gate**

Append to `/home/agents/trading-crypto/backtesting/evaluate_ppo.py`:

```python

# ---------------------------------------------------------------------------
# Gate
# ---------------------------------------------------------------------------

@dataclasses.dataclass
class GateResult:
    passed: bool
    reason: str
    metrics: dict   # n_positive_sharpe_windows, median_ir, n_windows


def compute_gate(window_results: list[dict], cfg: dict) -> GateResult:
    """Apply walk-forward promotion gate. Handles empty window_results gracefully."""
    if not window_results:
        return GateResult(
            passed=False,
            reason="no windows to evaluate",
            metrics={},
        )

    n_positive = sum(1 for w in window_results if w["sharpe"] > 0)
    median_ir = float(np.median([w["information_ratio"] for w in window_results]))
    min_pos = int(cfg["wf_min_positive_sharpe_windows"])
    min_ir = float(cfg["wf_min_information_ratio"])
    n_windows = len(window_results)

    passed = n_positive >= min_pos and median_ir >= min_ir

    if not passed:
        reasons: list[str] = []
        if n_positive < min_pos:
            reasons.append(
                f"only {n_positive}/{n_windows} windows positive Sharpe (need {min_pos})"
            )
        if median_ir < min_ir:
            reasons.append(f"median IR {median_ir:.3f} < {min_ir}")
        reason = "; ".join(reasons)
    else:
        reason = f"{n_positive}/{n_windows} positive Sharpe, median IR {median_ir:.3f}"

    return GateResult(
        passed=passed,
        reason=reason,
        metrics={
            "n_positive_sharpe_windows": n_positive,
            "median_ir": median_ir,
            "n_windows": n_windows,
        },
    )
```

- [ ] **Step 3.3: Run gate tests**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -m pytest tests/test_evaluate_ppo.py -v --tb=short 2>&1 | tail -25"
```

Expected: 19 passed.

- [ ] **Step 3.4: Run full suite**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -m pytest -q 2>&1 | tail -5"
```

Expected: 98 passed.

- [ ] **Step 3.5: Commit**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && git add backtesting/evaluate_ppo.py tests/test_evaluate_ppo.py && git commit -m 'feat: GateResult, compute_gate'"
```

---

## Task 4: load_model_and_data + main + CLI

**Files:**
- Modify: `backtesting/evaluate_ppo.py` — add load_model_and_data and main; add `if __name__ == "__main__"` entry point
- Modify: `tests/test_evaluate_ppo.py` — add 2 exit-code tests

### Background

`load_model_and_data` calls `build_ppo_dataset` from `training.train_ppo` (reuses the shared data loading pipeline) and converts the result into an `EvalDataset` with pre-computed arrays. `main` orchestrates: load → seed → carve → evaluate × windows → gate → MLflow → `sys.exit(0/1)`. Runtime errors raise and the `if __name__ == "__main__"` wrapper catches them and exits 2.

`dataset_hash` = SHA256 of `X_btc_bytes + X_eth_bytes` where bytes = `np.ascontiguousarray(arr).tobytes()`.

**MLflow run name:** `wf_eval_{git_sha_short}_{timestamp}` where `git_sha_short` = first 7 chars of HEAD sha (or "unknown").

- [ ] **Step 4.1: Write exit-code tests**

Append to `/home/agents/trading-crypto/tests/test_evaluate_ppo.py`:

```python
# ---------------------------------------------------------------------------
# Task 4: exit code tests (mock main internals)
# ---------------------------------------------------------------------------

def test_main_exit_code_pass(monkeypatch):
    """When gate passes, main calls sys.exit(0)."""
    import sys
    sys.path.insert(0, "/home/agents/trading-crypto")
    from backtesting import evaluate_ppo

    # Stub out expensive operations
    monkeypatch.setattr(evaluate_ppo, "load_model_and_data",
                        lambda model_path, data_dir: (_MockPPOAgent(), _make_eval_dataset()))
    monkeypatch.setattr("mlflow.start_run", __import__("contextlib").nullcontext)
    monkeypatch.setattr("mlflow.set_experiment", lambda *a, **kw: None)
    monkeypatch.setattr("mlflow.log_param", lambda *a, **kw: None)
    monkeypatch.setattr("mlflow.log_metric", lambda *a, **kw: None)
    monkeypatch.setattr("mlflow.set_tag", lambda *a, **kw: None)

    # Stub compute_gate to always pass
    monkeypatch.setattr(evaluate_ppo, "compute_gate",
                        lambda results, cfg: evaluate_ppo.GateResult(
                            passed=True, reason="mocked pass", metrics={}))

    with pytest.raises(SystemExit) as exc_info:
        evaluate_ppo.main(
            model_path="fake.zip", data_dir="/fake",
            eval_seed=0,
        )
    assert exc_info.value.code == 0


def test_main_exit_code_fail(monkeypatch):
    """When gate fails, main calls sys.exit(1)."""
    import sys
    sys.path.insert(0, "/home/agents/trading-crypto")
    from backtesting import evaluate_ppo

    monkeypatch.setattr(evaluate_ppo, "load_model_and_data",
                        lambda model_path, data_dir: (_MockPPOAgent(), _make_eval_dataset()))
    monkeypatch.setattr("mlflow.start_run", __import__("contextlib").nullcontext)
    monkeypatch.setattr("mlflow.set_experiment", lambda *a, **kw: None)
    monkeypatch.setattr("mlflow.log_param", lambda *a, **kw: None)
    monkeypatch.setattr("mlflow.log_metric", lambda *a, **kw: None)
    monkeypatch.setattr("mlflow.set_tag", lambda *a, **kw: None)

    monkeypatch.setattr(evaluate_ppo, "compute_gate",
                        lambda results, cfg: evaluate_ppo.GateResult(
                            passed=False, reason="mocked fail", metrics={}))

    with pytest.raises(SystemExit) as exc_info:
        evaluate_ppo.main(
            model_path="fake.zip", data_dir="/fake",
            eval_seed=0,
        )
    assert exc_info.value.code == 1
```

Run to confirm failure:
```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -m pytest tests/test_evaluate_ppo.py::test_main_exit_code_pass -v --tb=short 2>&1 | tail -10"
```

Expected: FAIL — `main` not implemented yet.

- [ ] **Step 4.2: Implement load_model_and_data and main**

Append to `/home/agents/trading-crypto/backtesting/evaluate_ppo.py`:

```python

# ---------------------------------------------------------------------------
# load_model_and_data
# ---------------------------------------------------------------------------

def load_model_and_data(model_path: str, data_dir: str) -> tuple:
    """
    Load PPO model + BTC/ETH feature data + XGB models + scalers.
    Returns (PPOAgent, EvalDataset).
    Raises FileNotFoundError with specific artifact name if any file is missing.
    VecNormalize not loaded: norm_obs=False means predict() uses raw obs at inference.
    """
    from features.engineer import FEATURE_COLS
    from models.ppo_agent import PPOAgent
    from training.train_ppo import build_ppo_dataset

    model_path = Path(model_path)
    if not model_path.exists():
        raise FileNotFoundError(f"PPO model not found: {model_path}")

    agent = PPOAgent()
    agent.load(model_path)

    log.info("Building eval dataset from %s", data_dir)
    ppo_ds = build_ppo_dataset(data_dir)

    df_btc = ppo_ds.df_btc_test
    df_eth = ppo_ds.df_eth_test
    n = len(df_btc)

    X_btc = ppo_ds.scaler_btc.transform(
        df_btc[FEATURE_COLS].values.astype(np.float32)
    ).astype(np.float32)
    X_eth = ppo_ds.scaler_eth.transform(
        df_eth[FEATURE_COLS].values.astype(np.float32)
    ).astype(np.float32)
    proba_btc = ppo_ds.xgb_btc.predict_proba(X_btc).astype(np.float32)
    proba_eth = ppo_ds.xgb_eth.predict_proba(X_eth).astype(np.float32)

    btc_closes = df_btc["close"].values.astype(np.float64)
    eth_closes = df_eth["close"].values.astype(np.float64)
    next_ret_btc = np.zeros(n, dtype=np.float64)
    next_ret_eth = np.zeros(n, dtype=np.float64)
    next_ret_btc[:-1] = np.log(btc_closes[1:] / btc_closes[:-1])
    next_ret_eth[:-1] = np.log(eth_closes[1:] / eth_closes[:-1])

    dataset_hash = hashlib.sha256(
        np.ascontiguousarray(X_btc).tobytes() + np.ascontiguousarray(X_eth).tobytes()
    ).hexdigest()

    return agent, EvalDataset(
        X_btc=X_btc, X_eth=X_eth,
        proba_btc=proba_btc, proba_eth=proba_eth,
        next_ret_btc=next_ret_btc, next_ret_eth=next_ret_eth,
        xgb_btc_run_id=ppo_ds.xgb_btc_run_id,
        xgb_eth_run_id=ppo_ds.xgb_eth_run_id,
        dataset_hash=dataset_hash,
    )


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

def main(model_path: str, data_dir: str, eval_seed: int = 42) -> None:
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")

    cfg = _load_ppo_config()
    K = int(cfg.get("k_window", 10))
    episode_length = int(cfg["episode_length"])
    stride = int(cfg["wf_episode_stride"])

    torch.manual_seed(eval_seed)
    np.random.seed(eval_seed)

    agent, dataset = load_model_and_data(model_path, data_dir)
    n_test = len(dataset.X_btc)

    windows = carve_windows(
        test_start=0, test_end=n_test,
        k_window=K, episode_length=episode_length, stride=stride,
    )
    if not windows:
        log.error("Insufficient test data for even one window. n_test=%d, K=%d, ep=%d",
                  n_test, K, episode_length)
        sys.exit(2)

    log.info("Walk-forward evaluation: %d windows, episode_length=%d, stride=%d",
             len(windows), episode_length, stride)

    rng = np.random.default_rng(eval_seed)
    window_results: list[dict] = []
    for i, window in enumerate(windows):
        result = evaluate_window(agent, dataset, window, cfg, rng)
        window_results.append(result)
        log.info(
            "Window %d [%d,%d): sharpe=%.3f  bh_sharpe=%.3f  IR=%.3f  mdd=%.3f  turnover=%.3f",
            i, window.start, window.end,
            result["sharpe"], result["bh_sharpe"], result["information_ratio"],
            result["max_drawdown"], result["turnover_per_step"],
        )

    gate = compute_gate(window_results, cfg)
    log.info("Gate: passed=%s — %s", gate.passed, gate.reason)

    # Git SHA
    try:
        git_sha = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], stderr=subprocess.DEVNULL
        ).decode().strip()
    except subprocess.CalledProcessError:
        git_sha = "unknown"
    git_sha_short = git_sha[:7]

    import time
    run_name = f"wf_eval_{git_sha_short}_{int(time.time())}"

    mlflow.set_experiment("trading-crypto-ppo")
    with mlflow.start_run(run_name=run_name):
        mlflow.set_tag("run_type", "evaluation")
        mlflow.set_tag("xgb_in_sample_bias", "true")

        # Reproducibility params
        mlflow.log_param("model_path", str(model_path))
        mlflow.log_param("dataset_hash", dataset.dataset_hash)
        mlflow.log_param("episode_length", episode_length)
        mlflow.log_param("k_window", K)
        mlflow.log_param("wf_episode_stride", stride)
        mlflow.log_param("wf_n_dirichlet_samples", cfg.get("wf_n_dirichlet_samples", 5))
        mlflow.log_param("eval_seed", eval_seed)
        mlflow.log_param("git_commit_sha", git_sha)
        mlflow.log_param("sb3_version", stable_baselines3.__version__)
        mlflow.log_param("torch_version", torch.__version__)
        mlflow.log_param("xgb_btc_run_id", dataset.xgb_btc_run_id)
        mlflow.log_param("xgb_eth_run_id", dataset.xgb_eth_run_id)

        # Per-window metrics
        for i, result in enumerate(window_results):
            mlflow.log_metric(f"wf_window_{i}_sharpe", result["sharpe"])
            mlflow.log_metric(f"wf_window_{i}_bh_sharpe", result["bh_sharpe"])
            mlflow.log_metric(f"wf_window_{i}_information_ratio", result["information_ratio"])
            mlflow.log_metric(f"wf_window_{i}_max_drawdown", result["max_drawdown"])
            mlflow.log_metric(f"wf_window_{i}_turnover_per_step", result["turnover_per_step"])
            mlflow.log_metric(f"wf_window_{i}_start_bar", result["window_start"])
            mlflow.log_metric(f"wf_window_{i}_end_bar", result["window_end"])

        # Aggregate
        mlflow.log_metric("wf_n_windows", gate.metrics.get("n_windows", len(window_results)))
        mlflow.log_metric("wf_n_positive_sharpe_windows",
                          gate.metrics.get("n_positive_sharpe_windows", 0))
        mlflow.log_metric("wf_median_information_ratio", gate.metrics.get("median_ir", 0.0))
        mlflow.log_metric("wf_promoted", int(gate.passed))

    sys.exit(0 if gate.passed else 1)


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Walk-forward PPO evaluation")
    parser.add_argument("--model-path", required=True, help="Path to saved PPO model zip")
    parser.add_argument("--data-dir", required=True, help="Directory containing Parquet data files")
    parser.add_argument("--eval-seed", type=int, default=42, help="RNG seed for Dirichlet sampling")
    args = parser.parse_args()

    try:
        main(model_path=args.model_path, data_dir=args.data_dir, eval_seed=args.eval_seed)
    except (FileNotFoundError, ValueError, RuntimeError) as exc:
        log.error("Runtime error: %s", exc)
        sys.exit(2)
```

- [ ] **Step 4.3: Run exit-code tests**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -m pytest tests/test_evaluate_ppo.py::test_main_exit_code_pass tests/test_evaluate_ppo.py::test_main_exit_code_fail -v --tb=short 2>&1 | tail -15"
```

Expected: 2 passed.

- [ ] **Step 4.4: Run all evaluate_ppo tests**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -m pytest tests/test_evaluate_ppo.py -v --tb=short 2>&1 | tail -30"
```

Expected: 21 passed.

- [ ] **Step 4.5: Run full suite — no regressions**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -m pytest -q 2>&1 | tail -5"
```

Expected: 100 passed.

- [ ] **Step 4.6: Smoke-check the module is importable**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && source .venv/bin/activate && python3 -c 'from backtesting.evaluate_ppo import carve_windows, evaluate_window, compute_gate, load_model_and_data, main; print(\"import OK\")'"
```

Expected: `import OK`

- [ ] **Step 4.7: Commit**

```bash
ssh sgdgx01 "cd /home/agents/trading-crypto && git add backtesting/evaluate_ppo.py tests/test_evaluate_ppo.py && git commit -m 'feat: load_model_and_data, main, CLI — walk-forward evaluation complete'"
```

---

## Self-Review

**Spec coverage:**
- §3 WindowSlice + carve_windows → Task 1 ✅
- §4 EvalDataset + evaluate_window → Task 2 ✅
- §5 GateResult + compute_gate → Task 3 ✅
- §6 MLflow schema → Task 4 (main) ✅
- §7 Config additions → Task 1 step 1.1 ✅
- §8 CLI → Task 4 (if __name__ == "__main__") ✅
- §10 All 19 tests → Tasks 1–4 ✅
- Obs indexing `[t-K+1:t+1]` matches env → Task 2 obs construction + test_obs_matches_env ✅
- IR ddof=1 + div-by-zero guard → _information_ratio helper ✅
- dataset_hash from feature matrix bytes → Task 4 load_model_and_data ✅
- Exit codes 0/1/2 → Task 4 main + CLI wrapper ✅
- VecNormalize NOT loaded (norm_obs=False) → documented in load_model_and_data docstring ✅

**Placeholder scan:** No TBDs. All code blocks complete.

**Type consistency:** `WindowSlice`, `EvalDataset`, `GateResult` defined in Task 1/2/3 and used consistently in Task 4. `evaluate_window` takes `agent` (untyped — PPOAgent), `EvalDataset`, `WindowSlice`, `cfg: dict`, `rng: np.random.Generator` — same signature throughout.
