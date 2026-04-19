# trading-crypto Phase 2b: Walk-Forward RL Evaluation

**Date:** 2026-04-19
**Status:** Approved (post-Opus review)

---

## 1. Problem & Goal

Phase 2a's promotion gate uses a single 80/20 train/test split. One lucky test period can pass the gate by chance. Phase 2b adds walk-forward evaluation: carve the test period into overlapping 252-bar windows, run the trained PPO agent deterministically on each, compare to buy-and-hold via information ratio, and gate promotion on results across ~5 windows instead of one.

**Not in scope (Phase 2b):** Optuna HPO for PPO, SubprocVecEnv vectorized training, FreqAI live wiring, XGB retraining on PPO-aligned splits.

---

## 2. Architecture

```
models/saved/btc_eth_ppo.zip     ─┐
configs/ppo_config.yaml          ─┤──► backtesting/evaluate_ppo.py ──► MLflow + stdout + exit code
data/raw/{BTC_USDT,ETH_USDT}.parquet ─┘
```

**Note:** VecNormalize stats are NOT needed at evaluation time. Phase 2a wraps training with `VecNormalize(norm_obs=False, norm_reward=True)` — reward normalisation is training-only and `predict()` is unaffected at inference. The policy network in `btc_eth_ppo.zip` takes raw obs directly.

**One new file:** `backtesting/evaluate_ppo.py` — standalone evaluation script. No changes to `training/train_ppo.py`, `models/ppo_env.py`, or `models/ppo_agent.py`.

Supporting changes:
- `configs/ppo_config.yaml` — four new walk-forward params
- `tests/test_evaluate_ppo.py` — new test file

---

## 3. Window Carving

### Split

Test period = last 20% of aligned BTC+ETH bars, with `label_n_bars` embargo at the train/test boundary. Same split as Phase 2a. All bar indices are absolute (into the full aligned dataset, not relative to test split start).

### WindowSlice

```python
@dataclasses.dataclass(frozen=True)
class WindowSlice:
    start: int   # inclusive — half-open convention [start, end)
    end: int     # exclusive
    n_steps: int

    def __post_init__(self):
        assert self.end - self.start == self.n_steps, (
            f"WindowSlice inconsistency: end-start={self.end-self.start}, n_steps={self.n_steps}"
        )
```

### Carving Formula

```python
# First usable bar: need K bars of lookback before first obs
# obs at bar t uses X[t-K+1 : t+1], so first t requires t-K+1 >= test_start → t >= test_start + K - 1
# Use test_start + K to be safe (consistent with Phase 2a evaluate_agent which starts at K)
first_usable = test_start + k_window
stride = cfg["wf_episode_stride"]        # default 126
episode_length = cfg["episode_length"]   # default 252

windows = []
s = first_usable
while s + episode_length <= test_end:   # strictly fits — no partial windows
    windows.append(WindowSlice(start=s, end=s + episode_length, n_steps=episode_length))
    s += stride
```

With test_start=absolute offset, ~880 test bars, K=10, episode_length=252, stride=126: **5 overlapping windows**. Remainder bars discarded.

If `test_end - test_start < k_window + episode_length`, `carve_windows` returns `[]` and `main` exits with code 2 (runtime error, distinct from gate-fail code 1).

---

## 4. Per-Window Evaluation

### Obs Construction

Obs at bar `t` uses the **same slice as `PortfolioPPOEnv._obs()`**:

```python
btc_window = X_btc[t - K + 1 : t + 1].flatten()   # K bars, inclusive of bar t
eth_window = X_eth[t - K + 1 : t + 1].flatten()
obs = np.concatenate([btc_window, eth_window, proba_btc[t], proba_eth[t], weights]).astype(np.float32)
```

`proba_btc[t]` and `proba_eth[t]` are XGB class probs computed from features up to bar `t` — no lookahead. `next_ret_btc[t]` is defined as `log(close[t+1] / close[t])` (forward return keyed by `t`), matching Phase 2a's convention.

### Rollout

Each window runs M=`wf_n_dirichlet_samples` (default 5) independent rollouts with different Dirichlet(α=1) initial weights. Per-window metrics are the mean across M samples (each metric computed per sample first, then averaged — Sharpe averaged across samples, not computed from pooled returns; this is a robustness measure over starting states, not an unbiased estimator).

```python
def evaluate_window(agent: PPOAgent, dataset: PPODataset,
                    window: WindowSlice, cfg: dict, rng: np.random.Generator) -> dict:
    """
    Runs wf_n_dirichlet_samples rollouts on window bars, returns mean metrics dict.
    rng must be seeded before call for reproducibility.
    """
```

For each sample:
1. `raw = rng.exponential(1.0, size=3); w0 = (raw / raw.sum()).astype(np.float32)` — Dirichlet(1,1,1)
2. `weights = w0`
3. For `t` in `range(window.start, window.end)`:
   - Build obs per §4 Obs Construction
   - `w_new = agent.predict(obs, deterministic=True)` — softmax applied inside `predict`
   - `portfolio_return = w_new[0]*next_ret_btc[t] + w_new[1]*next_ret_eth[t]`
   - `turnover = np.abs(w_new - weights).sum()` — full L1, no /2, matches env
   - `transaction_cost = turnover * (fee_rate + slippage_bps / 10_000)`
   - `net_return = portfolio_return - transaction_cost`
   - `bh_return = 0.5*next_ret_btc[t] + 0.5*next_ret_eth[t]`
   - Accumulate `net_returns`, `bh_returns`, `turnovers`; set `weights = w_new`
4. `sharpe = mean(net_returns) / std(net_returns, ddof=1) * sqrt(6*365)` if `std > 1e-10` else `0.0`
5. `max_drawdown`: from equity curve `exp(cumsum(net_returns))` — always in `[0, 1]`
6. `bh_sharpe = mean(bh_returns) / std(bh_returns, ddof=1) * sqrt(6*365)` if `std > 1e-10` else `0.0`
7. `diff = net_returns - bh_returns; ir = mean(diff) / std(diff, ddof=1)` if `std(diff) > 1e-10` else `0.0`

Per-window output dict:

```python
{
    "sharpe": float,
    "max_drawdown": float,
    "turnover_per_step": float,
    "bh_sharpe": float,
    "bh_max_drawdown": float,
    "information_ratio": float,   # mean(net_ret - bh_ret) / std(net_ret - bh_ret, ddof=1)
    "window_start": int,
    "window_end": int,
    "n_steps": int,
}
```

---

## 5. Gate

```python
@dataclasses.dataclass
class GateResult:
    passed: bool
    reason: str
    metrics: dict   # n_positive_sharpe_windows, median_ir, n_windows

def compute_gate(window_results: list[dict], cfg: dict) -> GateResult:
    if not window_results:
        return GateResult(passed=False, reason="no windows to evaluate", metrics={})
    n_positive = sum(1 for w in window_results if w["sharpe"] > 0)
    median_ir = float(np.median([w["information_ratio"] for w in window_results]))
    min_pos = cfg["wf_min_positive_sharpe_windows"]   # default 4
    min_ir  = cfg["wf_min_information_ratio"]          # default 0.0
    passed = (n_positive >= min_pos and median_ir >= min_ir)
    if not passed:
        reasons = []
        if n_positive < min_pos:
            reasons.append(f"only {n_positive}/{len(window_results)} windows positive Sharpe (need {min_pos})")
        if median_ir < min_ir:
            reasons.append(f"median IR {median_ir:.3f} < {min_ir}")
        reason = "; ".join(reasons)
    else:
        reason = f"{n_positive}/{len(window_results)} positive Sharpe, median IR {median_ir:.3f}"
    return GateResult(
        passed=passed, reason=reason,
        metrics={"n_positive_sharpe_windows": n_positive, "median_ir": median_ir,
                 "n_windows": len(window_results)},
    )
```

**Exit codes:** 0 = gate passed, 1 = gate failed, 2 = runtime error (missing files, insufficient data, shape mismatch).

**Statistical power note:** With ~5 overlapping windows (stride=126, length=252 → 50% overlap), `P(≥4/5 positive by chance) ≈ 19%` under a coin-flip independence assumption — but the overlapping windows violate independence, so the true false-positive rate is higher. This is a sanity check, not a rigorous hypothesis test. True statistical power requires Phase 2c's full retrain-per-fold CPCV.

---

## 6. MLflow Schema

Logs to experiment `trading-crypto-ppo`. Run name: `wf_eval_{git_sha_short}_{timestamp}`. Run tag: `run_type=evaluation`.

**Per-window metrics** (i = 0-indexed):
- `wf_window_{i}_sharpe`
- `wf_window_{i}_bh_sharpe`
- `wf_window_{i}_information_ratio`
- `wf_window_{i}_max_drawdown`
- `wf_window_{i}_turnover_per_step`
- `wf_window_{i}_start_bar`, `wf_window_{i}_end_bar`

**Aggregate:**
- `wf_n_windows`
- `wf_n_positive_sharpe_windows`
- `wf_median_information_ratio`
- `wf_promoted` (0 or 1)

**Reproducibility params:**
- `model_path`
- `dataset_hash` — SHA256 of `np.ascontiguousarray(X_btc_test_concat).tobytes()` (feature matrix bytes, not just timestamps)
- `episode_length`, `k_window`, `label_n_bars`, `wf_episode_stride`, `wf_n_dirichlet_samples`
- `eval_seed`
- `git_commit_sha`
- `sb3_version`, `torch_version`
- `xgb_in_sample_bias=true` — known Phase 2a bias; XGB probs on test bars are in-sample for XGB

---

## 7. Config (`configs/ppo_config.yaml` additions)

```yaml
# Walk-forward evaluation
wf_episode_stride: 126                 # bars between window starts (50% overlap with episode_length=252)
wf_min_positive_sharpe_windows: 4     # gate: Sharpe > 0 in at least this many windows
wf_min_information_ratio: 0.0         # gate: median window IR >= this (0.0 = must match B&H after costs)
wf_n_dirichlet_samples: 5             # Dirichlet(1,1,1) starts averaged per window
```

---

## 8. CLI

```bash
python backtesting/evaluate_ppo.py \
    --model-path models/saved/btc_eth_ppo.zip \
    --data-dir data/raw \
    [--eval-seed 42]
```

Prints per-window table + gate result to stdout. Exits 0/1/2.

---

## 9. Function Interfaces

```python
def load_model_and_data(model_path: str, data_dir: str) -> tuple[PPOAgent, PPODataset]:
    """Loads PPO model + BTC/ETH datasets + XGB models + scalers.
    Raises FileNotFoundError with specific artifact name if any file is missing.
    No VecNormalize needed: norm_obs=False in training means predict() uses raw obs."""

def carve_windows(test_start: int, test_end: int,
                  k_window: int, episode_length: int, stride: int) -> list[WindowSlice]:
    """Returns overlapping strided WindowSlice list. Returns [] if insufficient data."""

def evaluate_window(agent: PPOAgent, dataset: PPODataset,
                    window: WindowSlice, cfg: dict,
                    rng: np.random.Generator) -> dict:
    """Runs wf_n_dirichlet_samples rollouts, returns mean metrics dict."""

def compute_gate(window_results: list[dict], cfg: dict) -> GateResult:
    """Applies walk-forward gate. Handles empty window_results gracefully."""

def main(model_path: str, data_dir: str, eval_seed: int = 42) -> None:
    """load → seed torch+numpy → carve → evaluate × windows → gate → MLflow → sys.exit."""
```

---

## 10. Tests (`tests/test_evaluate_ppo.py`)

| Test | What it verifies |
|---|---|
| `test_carve_windows_correct_count` | Formula gives expected window count for known inputs |
| `test_carve_windows_stride_spacing` | Adjacent window starts differ by exactly stride |
| `test_carve_windows_half_open_invariant` | `window.end - window.start == window.n_steps` for all windows |
| `test_carve_windows_respects_warmup` | First window start == test_start + k_window |
| `test_carve_windows_insufficient_data` | Returns [] when data < k_window + episode_length |
| `test_obs_matches_env` | Obs built in evaluate_window matches `PortfolioPPOEnv._obs()` at same t |
| `test_evaluate_window_returns_correct_keys` | Result dict has all 9 expected keys |
| `test_evaluate_window_bh_is_50_50` | B&H return matches 0.5×btc + 0.5×eth over window bars |
| `test_evaluate_window_deterministic` | Same rng seed → identical per-window metrics |
| `test_evaluate_window_turnover_no_division_by_two` | Full BTC→ETH rotation incurs 2× fee (L1=2) |
| `test_information_ratio_zero_variance` | agent_returns == bh_returns → IR returns 0.0, no crash |
| `test_gate_passes_when_criteria_met` | 4/5 positive Sharpe, IR=0.5 → passed=True |
| `test_gate_fails_insufficient_positive_windows` | 3/5 positive → passed=False, reason cites count |
| `test_gate_fails_negative_median_ir` | Median IR < 0 → passed=False even if 5/5 positive Sharpe |
| `test_gate_result_reason_string` | GateResult.reason non-empty and describes failure |
| `test_gate_edge_case_exactly_at_threshold` | median_ir == 0.0 with `>=` passes |
| `test_gate_empty_windows` | compute_gate([]) → passed=False, no crash |
| `test_main_exit_code_pass` | Passing gate → sys.exit(0) |
| `test_main_exit_code_fail` | Failing gate → sys.exit(1) |

All tests use synthetic DataFrames and `_MockPPOAgent` returning fixed weights. No real Parquet files needed.

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
