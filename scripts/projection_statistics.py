"""Conservative adoption gates frozen by projection-comparison-protocol.md.

Python 3.11+, standard library only. Each input stratum has ``task``, the
frozen ``weight``, ``independent_blocks: True`` and a nonempty ``pairs`` list.
Each pair has literal Boolean ``baseline_success`` / ``candidate_success``.
Cost analysis additionally requires finite nonnegative ``baseline_cost`` /
``candidate_cost`` on *every* pair, positive predeclared per-Run
``baseline_cost_bound`` / ``candidate_cost_bound``, and
``cost_bounds_enforced: True``. An assertion of independence or enforcement
must come from the experiment registration; this module cannot establish it.

Quality uses paired gain/loss CP bounds, delta=alpha/(2*5). Cost uses bounded
empirical Bernstein and CP bounds, delta=alpha/(4*5). The lower quality and
upper cost bounds are the adoption gates. Opposite-direction bounds support
``fail`` only when they establish a regression; insufficient precision is
``inconclusive``. The two directions have separate one-sided error budgets,
and are not advertised as a simultaneous two-sided 97.5% interval.

All outputs are JSON-finite: an unbounded/undefined ratio is null with an
explicit reason, never a successful comparison. Unknown charges are not zero.
No observations are resampled for an adoption decision. The separate planner
uses only pilot rows, never supplies confirmation evidence, and never dispatches
provider work or changes a shared budget.
"""
from functools import lru_cache
import math
import random
import statistics


PRIMARY_WEIGHTS = {
    "code-change-hidden-regression": .125,
    "code-change-verified-fix": .125,
    "structured-data-overdue-totals": .25,
    "long-conversation-corrections": .25,
    "correction-lost-in-pruned-tool-result": .25,
}
CONFIRMATION_SIZES = tuple(range(50, 251, 25))
ROUTES = ("automatic", "explicit")


def _finite(value):
    try:
        return type(value) in (int, float) and math.isfinite(value)
    except (OverflowError, TypeError, ValueError):
        return False


def _beta_fraction(a, b, x):
    """Continued fraction for the regularized incomplete beta integral."""
    tiny = 1e-300
    qab, qap, qam = a + b, a + 1, a - 1
    c = 1.0
    d = 1 - qab * x / qap
    if abs(d) < tiny:
        d = tiny
    d = 1 / d
    value = d
    for m in range(1, 1001):
        m2 = 2 * m
        aa = m * (b - m) * x / ((qam + m2) * (a + m2))
        d = 1 + aa * d
        c = 1 + aa / c
        if abs(d) < tiny:
            d = tiny
        if abs(c) < tiny:
            c = tiny
        d = 1 / d
        value *= d * c
        aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2))
        d = 1 + aa * d
        c = 1 + aa / c
        if abs(d) < tiny:
            d = tiny
        if abs(c) < tiny:
            c = tiny
        d = 1 / d
        change = d * c
        value *= change
        if abs(change - 1) < 3e-14:
            return value
    raise ArithmeticError("incomplete beta failed to converge")


def _beta_cdf(a, b, x):
    if x <= 0:
        return 0.0
    if x >= 1:
        return 1.0
    factor = math.exp(math.lgamma(a + b) - math.lgamma(a) - math.lgamma(b)
                      + a * math.log(x) + b * math.log1p(-x))
    if x < (a + 1) / (a + b + 2):
        return factor * _beta_fraction(a, b, x) / a
    return 1 - factor * _beta_fraction(b, a, 1 - x) / b


def _beta_quantile(a, b, probability):
    lo, hi = 0.0, 1.0
    for _ in range(60):
        middle = (lo + hi) / 2
        if _beta_cdf(a, b, middle) < probability:
            lo = middle
        else:
            hi = middle
    return (lo + hi) / 2


@lru_cache(maxsize=32768)
def _cp(count, total, delta):
    lower = 0.0 if count == 0 else (
        delta ** (1 / total) if count == total
        else _beta_quantile(count, total - count + 1, delta))
    upper = 1.0 if count == total else (
        -math.expm1(math.log(delta) / total) if count == 0
        else _beta_quantile(count + 1, total - count, 1 - delta))
    return lower, upper


def clopper_pearson(count, total, delta=.025):
    """Exact one-sided lower and upper binomial limits (each error delta).

    Boolean or floating-point counts are invalid, even if numerically integral.
    Inversion uses the binomial/beta identity, without a normal approximation.
    """
    if (type(count) is not int or type(total) is not int or total <= 0
            or not 0 <= count <= total or not _finite(delta) or not 1e-12 <= delta < .5):
        raise ValueError("CP requires integer 0 <= count <= total, total > 0 and 1e-12 <= delta < .5")
    return _cp(count, total, delta)


def _validate(strata, alpha, costs=False):
    reasons, denominators = [], {}
    if not _finite(alpha) or not 2e-11 <= alpha < .5:
        reasons.append("alpha must be finite, at least 2e-11, and below .5 for supported numeric precision")
    if not isinstance(strata, list) or len(strata) != 5:
        return ["all five primary task strata are required"] + reasons, denominators
    seen = set()
    for entry in strata:
        if (not isinstance(entry, dict) or not isinstance(entry.get("task"), str)
                or entry["task"] not in PRIMARY_WEIGHTS):
            reasons.append("unknown or malformed primary task stratum")
            continue
        task = entry["task"]
        if task in seen:
            reasons.append(f"{task}: duplicate primary stratum")
        seen.add(task)
        if not _finite(entry.get("weight")) or entry["weight"] != PRIMARY_WEIGHTS[task]:
            reasons.append(f"{task}: weight differs from the frozen primary population")
        if entry.get("independent_blocks") is not True:
            reasons.append(f"{task}: independent repetition blocks have not been established")
        pairs = entry.get("pairs")
        if not isinstance(pairs, list) or not pairs:
            reasons.append(f"{task}: no matched repetition pairs")
            continue
        valid_pairs = all(isinstance(pair, dict)
                          and type(pair.get("baseline_success")) is bool
                          and type(pair.get("candidate_success")) is bool for pair in pairs)
        if not valid_pairs:
            reasons.append(f"{task}: successes must be literal booleans in every matched pair")
            continue
        denominators[task] = {"pairs": len(pairs),
                              "baseline_successes": sum(p["baseline_success"] for p in pairs),
                              "candidate_successes": sum(p["candidate_success"] for p in pairs)}
        if costs:
            if entry.get("cost_bounds_enforced") is not True:
                reasons.append(f"{task}: cost bounds are not established as predeclared and enforceable")
            for arm in ("baseline", "candidate"):
                bound = entry.get(arm + "_cost_bound")
                if not _finite(bound) or bound <= 0:
                    reasons.append(f"{task}: {arm} has no finite positive per-Run cost bound")
                    continue
                if any(not _finite(p.get(arm + "_cost")) or p[arm + "_cost"] < 0
                       for p in pairs):
                    reasons.append(f"{task}: {arm} cost is unknown, negative or non-finite")
                elif any(p[arm + "_cost"] > bound for p in pairs):
                    reasons.append(f"{task}: observed {arm} cost exceeds the predeclared bound")
            if len(pairs) < 2:
                reasons.append(f"{task}: empirical Bernstein requires at least two repetitions")
    if seen != set(PRIMARY_WEIGHTS):
        reasons.append("primary task population is incomplete")
    return reasons, denominators


def _inconclusive(reasons, denominators=None):
    return {"status": "inconclusive", "bounds": {"lower": None, "upper": None},
            "reasons": reasons, "denominators": denominators or {}, "strata": []}


def _quality_limits(gains, losses, n, delta):
    lg, ug = _cp(gains, n, delta)
    ll, ul = _cp(losses, n, delta)
    return lg - ul, ug - ll


def quality_bound(strata, alpha=.025):
    """Bound fixed-weight candidate-minus-baseline success probability.

    Adoption requires lower > -.05; the equality boundary is inconclusive.
    Costs do not enter this calculation, so missing cost evidence need not
    hide an independently observable quality regression.
    """
    reasons, denominators = _validate(strata, alpha)
    if reasons:
        return _inconclusive(reasons, denominators)
    lower = upper = difference = 0.0
    details = []
    delta = alpha / 10
    for entry in strata:
        pairs = entry["pairs"]
        gains = sum(not p["baseline_success"] and p["candidate_success"] for p in pairs)
        losses = sum(p["baseline_success"] and not p["candidate_success"] for p in pairs)
        lo, hi = _quality_limits(gains, losses, len(pairs), delta)
        weight = entry["weight"]
        lower += weight * lo
        upper += weight * hi
        difference += weight * (gains - losses) / len(pairs)
        details.append({"task": entry["task"], "gain_count": gains, "loss_count": losses,
                        "gain_lower": _cp(gains, len(pairs), delta)[0],
                        "loss_upper": _cp(losses, len(pairs), delta)[1],
                        "lower": lo, "upper": hi})
    status = "pass" if lower > -.05 else "fail" if upper < -.05 else "inconclusive"
    if status == "inconclusive":
        reasons.append("quality lower bound does not exceed the strict -.05 non-inferiority threshold")
    return {"status": status, "bounds": {"lower": lower, "upper": upper},
            "estimates": {"success_difference": difference}, "alpha": alpha,
            "allocated_error": delta, "reasons": reasons,
            "denominators": denominators, "strata": details}


def _cost_limits(mean, normalized_variance, bound, n, delta):
    # Work in units of B to avoid overflowing the sample variance of large
    # finite cost units. This is algebraically the frozen Bernstein radius.
    log_term = math.log(2 / delta)
    radius = math.sqrt(2 * normalized_variance * log_term / n) + 7 * log_term / (3 * (n - 1))
    return (bound * max(0.0, mean / bound - radius),
            bound * min(1.0, mean / bound + radius))


def _ratio(numerator_cost, numerator_success, denominator_cost, denominator_success):
    if denominator_cost <= 0 or denominator_success <= 0:
        return None
    value = numerator_cost / denominator_cost * (numerator_success / denominator_success)
    return value if math.isfinite(value) else None


def _cost_per_success(mean_cost, success):
    if success <= 0:
        return None
    value = mean_cost / success
    return value if math.isfinite(value) else None


def cost_ratio_bound(strata, alpha=.025):
    """Bound candidate / baseline cost per success, retaining failed costs.

    A zero lower denominator makes the upper bound unbounded (JSON null).
    Adoption requires the upper bound to be strictly less than .90.
    """
    reasons, denominators = _validate(strata, alpha, costs=True)
    if reasons:
        return _inconclusive(reasons, denominators)
    delta = alpha / 20
    totals = {arm: {"cost_lower": 0.0, "cost_upper": 0.0, "success_lower": 0.0,
                    "success_upper": 0.0, "mean_cost": 0.0, "success": 0.0}
              for arm in ("baseline", "candidate")}
    details = []
    for entry in strata:
        n, weight = len(entry["pairs"]), entry["weight"]
        detail = {"task": entry["task"]}
        for arm in ("baseline", "candidate"):
            costs = [pair[arm + "_cost"] for pair in entry["pairs"]]
            bound = entry[arm + "_cost_bound"]
            mean = statistics.fmean(costs)
            variance = statistics.variance(cost / bound for cost in costs)
            lo, hi = _cost_limits(mean, variance, bound, n, delta)
            successes = denominators[entry["task"]][arm + "_successes"]
            ls, us = _cp(successes, n, delta)
            values = {"cost_lower": lo, "cost_upper": hi, "success_lower": ls,
                      "success_upper": us, "mean_cost": mean, "success": successes / n}
            for name, value in values.items():
                totals[arm][name] += weight * value
            detail[arm] = values
        details.append(detail)
    b, c = totals["baseline"], totals["candidate"]
    upper = _ratio(c["cost_upper"], b["success_upper"], b["cost_lower"], c["success_lower"])
    lower = _ratio(c["cost_lower"], b["success_lower"], b["cost_upper"], c["success_upper"])
    if upper is None:
        reasons.append("zero lower denominator or non-finite upper cost-ratio bound")
    # A zero-success arm cannot supply a usable cost-per-success ratio.
    baseline_cost_per_success = _cost_per_success(b["mean_cost"], b["success"])
    candidate_cost_per_success = _cost_per_success(c["mean_cost"], c["success"])
    defined = baseline_cost_per_success is not None and candidate_cost_per_success is not None
    status = ("pass" if defined and upper is not None and upper < .9 else
              "fail" if defined and lower is not None and lower > .9 else "inconclusive")
    if not defined:
        reasons.append("at least one arm has a zero successful-task denominator or non-finite cost per success")
    elif status == "inconclusive" and upper is not None:
        reasons.append("cost-ratio upper bound does not fall below the strict .90 threshold")
    return {"status": status,
            "bounds": {"lower": lower, "upper": upper,
                       "candidate_mean_cost_upper": c["cost_upper"],
                       "baseline_mean_cost_lower": b["cost_lower"],
                       "baseline_success_upper": b["success_upper"],
                       "candidate_success_lower": c["success_lower"]},
            "estimates": {"baseline_cost_per_success": baseline_cost_per_success,
                          "candidate_cost_per_success": candidate_cost_per_success},
            "alpha": alpha, "allocated_error": delta, "reasons": reasons,
            "denominators": denominators, "strata": details}


def _covariance(rows):
    names = ("baseline_success", "candidate_success", "baseline_cost", "candidate_cost")
    columns = [[float(row[name]) for row in rows] for name in names]
    means = [statistics.fmean(column) for column in columns]
    matrix = []
    for i, column in enumerate(columns):
        matrix.append([sum((a - means[i]) * (b - means[j]) for a, b in zip(column, other))
                       / (len(rows) - 1) for j, other in enumerate(columns)])
    if any(not math.isfinite(value) for row in matrix for value in row):
        raise ValueError("non-finite pilot covariance")
    return {"variables": list(names), "sample_covariance": matrix}


def _planning_rows(entry):
    """Symmetric working alternative and a declared discordance sensitivity.

    Random arm swapping of each complete empirical pair preserves the paired
    cost/success association under exchangeability; scaling the candidate
    cost by .80 imposes the planning economic alternative. Equal marginal
    success probabilities impose difference zero. This is a model assumption,
    never an inferred provider improvement. Original covariance is reported.

    Sensitivity increases discordance to its one-sided 99.5% exact pilot
    upper limit (Bonferroni .025/5), preserving the symmetric marginals by
    removing equal mass from both concordant states. If the empirical success
    marginal is degenerate, that construction cannot support uncertainty and
    planning is refused instead of inventing favorable trials.
    """
    rows, n = entry["pairs"], len(entry["pairs"])
    both = sum(p["baseline_success"] and p["candidate_success"] for p in rows) / n
    neither = sum(not p["baseline_success"] and not p["candidate_success"] for p in rows) / n
    discordant = sum(p["baseline_success"] != p["candidate_success"] for p in rows)
    if both == 1 or neither == 1:
        raise ValueError(f"{entry['task']}: pilot cannot support a useful planning distribution; "
                         "ten identical success ties do not establish zero discordance")
    upper = _cp(discordant, n, .025 / 5)[1]
    observed = discordant / n
    maximum = observed + 2 * min(both, neither)
    if maximum < upper:
        # Capping the sensitivity at empirical marginal feasibility would hide
        # uncertainty in those marginals, so do not silently use that shortcut.
        raise ValueError(f"{entry['task']}: planning distribution cannot accommodate the exact "
                         "discordance sensitivity without inventing a success marginal")
    transfer = (upper - observed) / 2
    for row in rows:
        if max(row["baseline_cost"], row["candidate_cost"]) > entry["baseline_cost_bound"]:
            raise ValueError(f"{entry['task']}: symmetrized planning costs exceed baseline bound")
        if .8 * max(row["baseline_cost"], row["candidate_cost"]) > entry["candidate_cost_bound"]:
            raise ValueError(f"{entry['task']}: symmetrized planning costs exceed candidate bound")
    return {"task": entry["task"], "observed_discordance": observed,
            "discordance_upper": upper, "sensitivity_error": .025 / 5,
            "convert_both_probability": transfer / both if both else 0.0,
            "convert_neither_probability": transfer / neither if neither else 0.0}


def _simulate_route(strata, sensitivities, simulations, rng):
    """Draw paired blocks once to n=250, score every fixed candidate size.

    Moment summaries are sufficient for the *same* CP/Bernstein gate formulas;
    no normal approximation or surrogate gate is used. Common random numbers
    across sample sizes improve reproducibility without changing their power.
    """
    counts = {n: {"quality": 0, "cost": 0} for n in CONFIRMATION_SIZES}
    qdelta, cdelta = .025 / 10, .025 / 20
    # Cache all attainable count bounds; simulation only performs lookups.
    tables = {n: {delta: [_cp(k, n, delta) for k in range(n + 1)]
                  for delta in (qdelta, cdelta)} for n in CONFIRMATION_SIZES}
    for _ in range(simulations):
        aggregate = {n: [0.0, 0.0, 0.0, 0.0, 0.0] for n in CONFIRMATION_SIZES}
        for entry, sensitivity in zip(strata, sensitivities):
            # success counts, gain/loss counts, normalized cost moments.
            bs = cs = gains = losses = 0
            bm = cm = bm2 = cm2 = 0.0
            bb, cb = entry["baseline_cost_bound"], entry["candidate_cost_bound"]
            rows, weight = entry["pairs"], entry["weight"]
            for n in range(1, 251):
                row = rows[rng.randrange(len(rows))]
                b, c = row["baseline_success"], row["candidate_success"]
                bc, cc = row["baseline_cost"], row["candidate_cost"]
                if rng.getrandbits(1):
                    b, c, bc, cc = c, b, cc, bc
                cc *= .8
                if sensitivity:
                    probability = (sensitivity["convert_both_probability"] if b and c else
                                   sensitivity["convert_neither_probability"] if not b and not c else 0.0)
                    if rng.random() < probability:
                        b = bool(rng.getrandbits(1))
                        c = not b
                bs += b
                cs += c
                gains += c and not b
                losses += b and not c
                # Welford in units of the enforceable bound.
                db, dc = bc / bb - bm, cc / cb - cm
                bm += db / n
                cm += dc / n
                bm2 += db * (bc / bb - bm)
                cm2 += dc * (cc / cb - cm)
                if n not in aggregate:
                    continue
                qtable, ctable = tables[n][qdelta], tables[n][cdelta]
                values = aggregate[n]
                values[0] += weight * (qtable[gains][0] - qtable[losses][1])
                values[1] += weight * _cost_limits(cm * cb, max(0.0, cm2 / (n - 1)), cb, n, cdelta)[1]
                values[2] += weight * ctable[bs][1]
                values[3] += weight * _cost_limits(bm * bb, max(0.0, bm2 / (n - 1)), bb, n, cdelta)[0]
                values[4] += weight * ctable[cs][0]
        for n, (qlower, ccost, bsuccess, bcost, csuccess) in aggregate.items():
            counts[n]["quality"] += qlower > -.05
            ratio = _ratio(ccost, bsuccess, bcost, csuccess)
            counts[n]["cost"] += ratio is not None and ratio < .9
    return counts


def choose_confirmation_n(pilot, *, seed=181, simulations=10000, budget=None):
    """Freeze the smallest powered n, or give an explicit inconclusive result.

    ``pilot={"routes": [{"route": "automatic", "strata": [...]},
    {"route": "explicit", "strata": [...]}]}``, with ten pairs per task.
    Evaluate n=50..250 step25 and 10,000 simulations per size, separately for
    the empirical symmetric alternative and its discordance sensitivity.
    All four tests in *both* scenarios must have a one-sided 95% CP lower
    pass-probability bound >= .95. These powers are conditional on the recorded
    model assumptions; they are not guaranteed provider performance.

    Optional ``budget`` is a read-only numeric mapping with
    remaining_nanocredits, remaining_seconds, per_run_nanocredits and
    per_run_seconds. It covers the original eight tasks * two arms * two
    routes (32*n Runs). The caller's SharedBudget must additionally cover
    registered diagnostic/deployment-mode work and existing reservations.
    This helper does not reserve, dispatch, or approve anything.
    """
    result = {"status": "inconclusive", "selected_n": None, "required_runs": None,
              "bounds": {}, "reasons": [], "denominators": {}, "power_by_n": [],
              "seed": seed if type(seed) is int else None, "simulations": 10000,
              "candidate_sizes": list(CONFIRMATION_SIZES), "pilot_covariance": {},
              "sensitivity": {}, "planning_assumptions": {
                  "quality_difference": 0.0, "cost_ratio": .8,
                  "method": "paired empirical rows with random complete arm swap; candidate cost scaled .8",
                  "sensitivity": "exact discordance upper limit; equal mass from both concordant states",
                  "power": "conditional on planning assumptions; not a guarantee",
                  "required_power_lower": .95, "power_error": .05}}
    reasons = result["reasons"]
    if type(seed) is not int or type(simulations) is not int or simulations != 10000:
        reasons.append("planning requires an integer fixed seed and exactly 10000 simulations per size")
        return result
    routes = pilot.get("routes") if isinstance(pilot, dict) else None
    if (not isinstance(routes, list) or len(routes) != 2 or
            not all(isinstance(route, dict) and isinstance(route.get("route"), str) for route in routes)
            or {route["route"] for route in routes} != set(ROUTES)):
        reasons.append("pilot must contain both distinct required routes: automatic and explicit")
        return result
    routes = [dict(route) for route in sorted(routes, key=lambda r: ROUTES.index(r["route"]))]
    for route in routes:
        name = route["route"]
        errors, denominators = _validate(route.get("strata"), .025, costs=True)
        result["denominators"][name] = denominators
        reasons.extend(f"{name}: {error}" for error in errors)
        if any(value["pairs"] != 10 for value in denominators.values()):
            reasons.append(f"{name}: planning requires the frozen ten-pair pilot per task")
        if errors:
            continue
        # A canonical task order makes the fixed seed independent of JSON order.
        route["strata"] = sorted(route["strata"], key=lambda s: list(PRIMARY_WEIGHTS).index(s["task"]))
        result["pilot_covariance"][name] = {}
        result["sensitivity"][name] = []
        for entry in route["strata"]:
            try:
                result["pilot_covariance"][name][entry["task"]] = _covariance(entry["pairs"])
                result["sensitivity"][name].append(_planning_rows(entry))
            except (ValueError, ArithmeticError, OverflowError) as error:
                reasons.append(f"{name}: {error}")
    if budget is not None:
        keys = ("remaining_nanocredits", "remaining_seconds", "per_run_nanocredits", "per_run_seconds")
        if not isinstance(budget, dict) or any(not _finite(budget.get(key)) or budget[key] < 0 for key in keys):
            reasons.append("planning budget must supply finite nonnegative remaining money/time and per-Run ceilings")
        elif budget["per_run_nanocredits"] <= 0 or budget["per_run_seconds"] <= 0:
            reasons.append("planning per-Run money/time ceilings must be positive")
    if reasons:
        return result
    rng = random.Random(seed)
    simulations_by_route = {}
    for route in routes:
        name = route["route"]
        simulations_by_route[name] = {
            "empirical": _simulate_route(route["strata"], [None] * 5, simulations, rng),
            "discordance_sensitivity": _simulate_route(route["strata"], result["sensitivity"][name], simulations, rng),
        }
    for n in CONFIRMATION_SIZES:
        powers = {}
        qualifies = True
        for name in ROUTES:
            powers[name] = {}
            for scenario, counts in simulations_by_route[name].items():
                powers[name][scenario] = {}
                for test in ("quality", "cost"):
                    count = counts[n][test]
                    lower = _cp(count, simulations, .05)[0]
                    powers[name][scenario][test] = {"passes": count, "simulations": simulations,
                                                   "estimated_power": count / simulations,
                                                   "power_lower": lower}
                    qualifies = qualifies and lower >= .95
        result["power_by_n"].append({"n": n, "tests": powers, "qualifies": qualifies})
        if qualifies and result["selected_n"] is None:
            result["selected_n"], result["required_runs"] = n, 32 * n
    if result["selected_n"] is None:
        reasons.append("no frozen sample size through 250 meets the four power gates under both planning scenarios")
        return result
    n_runs = result["required_runs"]
    if budget is not None:
        money = n_runs * budget["per_run_nanocredits"]
        seconds = n_runs * budget["per_run_seconds"]
        if not _finite(money) or not _finite(seconds):
            reasons.append("complete confirmation has a non-finite aggregate budget requirement")
            result["statistically_selected_n"] = result["selected_n"]
            result["selected_n"] = None
            return result
        result["budget_requirement"] = {"nanocredits": money, "seconds": seconds,
                                        "includes_extra_diagnostics_or_deployment": False}
        if money > budget["remaining_nanocredits"] or seconds > budget["remaining_seconds"]:
            reasons.append("complete powered confirmation exceeds the remaining shared money/time budget")
            result["statistically_selected_n"] = result["selected_n"]
            result["selected_n"] = None
            return result
    result["status"] = "pass"
    return result
