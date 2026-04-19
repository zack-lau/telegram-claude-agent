# trading-crypto Phase 2b: Walk-Forward RL Evaluation

**Date:** 2026-04-19
**Status:** Draft

---

## 1. Problem & Goal

Phase 2a's promotion gate uses a single 80/20 train/test split. One lucky test period can pass the gate by chance. Phase 2b adds walk-forward evaluation: carve the test period into overlapping 252-bar windows, run the trained PPO agent deterministically on each, compare to buy-and-hold via information ratio, and gate promotion on results across ~5 windows instead of one.

**Not in scope (Phase 2b):** Optuna HPO for PPO, SubprocVecEnv vectorized training, FreqAI live wiring, XGB retraining on PPO-aligned splits.

---

## 2. Architecture

```
models/saved/btc_eth_ppo.zip          ─┐
models/saved/btc_eth_ppo_vecnormalize.pkl ─┤
configs/ppo_config.yaml               ─┤──► backtesting/evaluate_ppo.py ──► MLflow + stdout + exit code
data/raw/{BTC_USDT,ETH_USDT}.parquet  ─┘
```

**One new file:** `backtesting/evaluate_ppo.py` — standalone evaluation script. No changes to `training/train_ppo.py`, `models/ppo_env.py`, or `models/ppo_agent.py`.

Supporting changes:
- `configs/ppo_config.yaml` — four new walk-forward params
- `tests/test_evaluate_ppo.py` — new test file

---

## 3. Window Carving

### Split

Test period = last 20% of aligned BTC+ETH bars, with `label_n_bars` embargo at the train/test boundary. Same split as Phase 2a.

### WindowSlice

```python
@dataclasses.dataclass(frozen=True)
class WindowSlice:
    start: int   # inclusive, half-open convention [start, end)
    end: int     # exclusive
    n_steps: int

    def __post_init__(self):
        assert self.end - self.start == self.n_steps, (
            f"WindowSlice inconsistency: end-start={self.end-self.start}, n_steps={self.n_steps}"
        )
```

All window indices use half-open convention `[start, end)`. `start` is the bar index into the full aligned dataset (not relative to test split start).

### Carving Formula

```python
# First usable bar: test_start + K (need K bars of lookback for first obs)
first_usable = test_start + k_window
stride = cfg["wf_episode_stride"]           # default 126
episode_length = cfg["episode_length"]       # default 252

windows = []
s = first_usable
while s + episode_length <= test_end:        # strictly fits
    windows.append(WindowSlice(start=s, end=s + episode_length, n_steps=episode_length))
    s += stride
```

With ~880 test bars, K=10, episode_length=252, stride=126: **5 windows** covering bars [10,262), [136,388), [262,514), [388,640), [514,766). Remainder bars discarded — no partial windows.

If `n_test_bars < K + episode_length`, `carve_windows` returns `[]` and `main` exits with an error.

---

## 4. Per-Window Evaluation

### Rollout

Each window runs M=`wf_n_dirichlet_samples` (default 5) independent rollouts with different Dirichlet(α=1) initial weights. Reports mean metrics across samples. Using Dirichlet samples (not fixed [1/3,1/3,1/3]) matches the training distribution, avoiding evaluation bias at the simplex centroid.

```python
def evaluate_window(agent: PPOAgent, dataset: PPODataset,
                    window: WindowSlice, cfg: dict, rng: np.random.Generator) -> dict:
    """
    Returns mean metrics across wf_n_dirichlet_samples Dirichlet-sampled starts.
    rng is a seeded Generator for reproducibility.
    """
```

For each sample:
1. Sample `raw = rng.exponential(1.0, size=3); w0 = raw / raw.sum()` (Dirichlet(1,1,1))
2. Set `weights = w0`
3. For `t` in `[window.start, window.end)`:
   - Build obs: `concat([X_btc[t-K:t].flatten(), X_eth[t-K:t].flatten(), proba_btc[t], proba_eth[t], weights])`
   - `w_new = agent.predict(obs, deterministic=True)` (softmax applied inside `predict`)
   - `portfolio_return = w_new[0]*next_ret_btc[t] + w_new[1]*next_ret_eth[t]`
   - `turnover = |w_new - weights|.sum()`
   - `transaction_cost = turnover * (fee_rate + slippage_bps/10_000)`
   - `net_return = portfolio_return - transaction_cost`
   - Accumulate net returns, weights
4. Compute Sharpe from net returns (annualized: `mean/std * sqrt(6*365)` for 4h bars)
5. Compute max drawdown from equity curve `exp(cumsum(net_returns))`
6. Compute B&H: `bh_returns = 0.5*next_ret_btc[t] + 0.5*next_ret_eth[t]` for same bars
7. Compute information ratio: `mean(net_returns - bh_returns) / std(net_returns - bh_returns)`

Per-window output (each metric computed per sample then averaged — not computed from pooled returns):

```python
{
    "sharpe": float,
    "max_drawdown": float,
    "turnover_per_step": float,
    "bh_sharpe": float,
    "bh_max_drawdown": float,
    "information_ratio": float,      # mean(agent_ret - bh_ret) / std(agent_ret - bh_ret)
    "window_start": int,
    "window_end": int,
    "n_steps": int,
}
```

`deterministic=True` is required — matches Phase 2a's `evaluate_agent` which also uses `deterministic=True`.

---

## 5. Gate

```python
@dataclasses.dataclass
class GateResult:
    passed: bool
    reason: str
    metrics: dict   # n_positive, median_ir, n_windows, per-window results

def compute_gate(window_results: list[dict], cfg: dict) -> GateResult:
    n_positive = sum(1 for w in window_results if w["sharpe"] > 0)
    median_ir = float(np.median([w["information_ratio"] for w in window_results]))
    min_pos = cfg["wf_min_positive_windows"]       # default 4
    min_ir  = cfg["wf_min_information_ratio"]       # default 0.0
    passed = (n_positive >= min_pos and median_ir >= min_ir)
    if not passed:
        reasons = []
        if n_positive < min_pos:
            reasons.append(f"only {n_positive}/{len(window_results)} windows positive Sharpe (need {min_pos})")
        if median_ir < min_ir:
            reasons.append(f"median IR {median_ir:.3f} < {min_ir}")
        reason = "; ".join(reasons)
    else:
        reason = f"{n_positive}/{len(window_results)} positive, median IR {median_ir:.3f}"
    return GateResult(passed=passed, reason=reason,
                      metrics={"n_positive": n_positive, "median_ir": median_ir,
                               "n_windows": len(window_results)})
```

Exit code 0 = passed, exit code 1 = failed.

**Statistical power note:** With ~5 windows, `P(≥4/5 positive by chance) ≈ 19%` under a coin-flip null. This is a sanity check, not a rigorous hypothesis test. The information ratio gate adds a second dimension. True statistical power requires Phase 2c's full retrain-per-fold CPCV.

---

## 6. MLflow Schema

Logs to experiment `trading-crypto-ppo`, run type tagged `evaluation` (not `training`).

**Per-window metrics** (i = 0-indexed window number):
- `wf_window_{i}_sharpe`
- `wf_window_{i}_bh_sharpe`
- `wf_window_{i}_information_ratio`
- `wf_window_{i}_max_drawdown`
- `wf_window_{i}_turnover_per_step`
- `wf_window_{i}_start_bar`, `wf_window_{i}_end_bar`

**Aggregate:**
- `wf_n_windows`
- `wf_n_positive`
- `wf_median_information_ratio`
- `wf_promoted` (bool as int 0/1)

**Reproducibility tags:**
- `model_path`
- `vec_normalize_path`
- `dataset_hash` (SHA256 of aligned test-split bar timestamps — verifies same data was used)
- `episode_length`, `k_window`, `label_n_bars`, `wf_episode_stride`, `wf_n_dirichlet_samples`
- `eval_seed` (seed used for Dirichlet sampling rng)
- `git_commit_sha`
- `sb3_version`, `torch_version`
- `xgb_in_sample_bias: true` (documents the known Phase 2a bias where XGB probs on test bars are technically in-sample for XGB)

---

## 7. Config (`configs/ppo_config.yaml` additions)

```yaml
# Walk-forward evaluation
wf_episode_stride: 126          # bars between window starts (overlap = episode_length - stride)
wf_min_positive_windows: 4     # gate: agent Sharpe > 0 in at least this many windows
wf_min_information_ratio: 0.0  # gate: median window IR must be >= this (0.0 = must beat B&H)
wf_n_dirichlet_samples: 5      # Dirichlet(1,1,1) starts averaged per window for robust eval
```

---

## 8. CLI

```bash
python backtesting/evaluate_ppo.py \
    --model-path models/saved/btc_eth_ppo.zip \
    --data-dir data/raw \
    [--eval-seed 42]          # default 42
```

Prints per-window table + gate result to stdout. Exits 0/1.

---

## 9. Function Interfaces

```python
def load_model_and_data(model_path: str, data_dir: str) -> tuple[PPOAgent, PPODataset]:
    """Loads PPO model + VecNormalize stats + BTC/ETH datasets + XGB models + scalers."""

def carve_windows(n_test_bars: int, test_start: int, test_end: int,
                  k_window: int, episode_length: int, stride: int) -> list[WindowSlice]:
    """Returns list of non-overlapping (strided) WindowSlice objects. Returns [] if insufficient data."""

def evaluate_window(agent: PPOAgent, dataset: PPODataset,
                    window: WindowSlice, cfg: dict,
                    rng: np.random.Generator) -> dict:
    """Runs M Dirichlet-sampled rollouts on the window, returns mean metrics dict."""

def compute_gate(window_results: list[dict], cfg: dict) -> GateResult:
    """Applies walk-forward promotion gate, returns GateResult with reason string."""

def main(model_path: str, data_dir: str, eval_seed: int = 42) -> None:
    """Orchestrates load → carve → evaluate × windows → gate → MLflow → exit."""
```

---

## 10. Tests (`tests/test_evaluate_ppo.py`)

| Test | What it verifies |
|---|---|
| `test_carve_windows_correct_count` | Formula gives expected window count for known inputs |
| `test_carve_windows_no_overlap_in_starts` | Adjacent window starts differ by exactly stride |
| `test_carve_windows_half_open` | `window.end - window.start == window.n_steps` (WindowSlice invariant) |
| `test_carve_windows_respects_embargo` | First window start ≥ test_start + k_window |
| `test_carve_windows_insufficient_data` | Returns [] when n_test_bars < k_window + episode_length |
| `test_evaluate_window_returns_correct_keys` | Result dict has all 9 expected keys |
| `test_evaluate_window_bh_is_50_50` | B&H return matches 0.5×btc + 0.5×eth over window bars |
| `test_evaluate_window_deterministic` | Same seed → identical per-window metrics |
| `test_evaluate_window_turnover_formula` | Full rotation uses full L1, no /2 (matches env) |
| `test_gate_passes_when_criteria_met` | 4/5 positive, IR=0.5 → passed=True |
| `test_gate_fails_insufficient_positive_windows` | 3/5 positive → passed=False, reason mentions count |
| `test_gate_fails_negative_median_ir` | Median IR < 0 → passed=False even if 5/5 windows positive |
| `test_gate_result_reason_string` | GateResult.reason is non-empty and describes failure |
| `test_gate_edge_case_exactly_at_threshold` | median_ir == 0.0 with `>=` passes |
| `test_load_model_loads_vec_normalize` | VecNormalize pkl loaded alongside model (no silent failure) |
| `test_main_exit_code_pass` | Passing gate → sys.exit(0) |
| `test_main_exit_code_fail` | Failing gate → sys.exit(1) |

All tests use synthetic DataFrames and a `_MockPPOAgent` returning fixed weights. No real Parquet files needed.

---

## 11. File Map

| File | Status |
|---|---|
| `backtesting/evaluate_ppo.py` | New |
| `configs/ppo_config.yaml` | Modify — add 4 walk-forward params |
| `tests/test_evaluate_ppo.py` | New |

---

## 12. What's Explicitly Out of Scope (Phase 2b)

- Full retrain-per-fold CPCV for RL (Phase 2c)
- Optuna hyperparameter tuning for PPO
- SubprocVecEnv vectorized training
- FreqAI live execution wiring
- XGB retraining on PPO-aligned splits
- Bootstrap confidence intervals on walk-forward metrics
- Per-window equity curve artifacts in MLflow
