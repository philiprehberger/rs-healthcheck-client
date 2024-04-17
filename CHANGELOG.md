# Changelog

## 0.1.1 (2026-03-22)

- Fix README compliance

## 0.1.0 (2026-03-20)

- `HealthChecker` with HTTP and TCP health checks
- `add_http()` and `add_tcp()` with default timeouts, `add_http_with()` and `add_tcp_with()` for custom settings
- `add_custom()` for user-defined check functions
- `check_all()` runs all checks in parallel, returns `HealthReport`
- `check_one()` runs a single named check
- `HealthReport` with `overall` status, `is_healthy()`, `unhealthy_checks()`, `summary()`, and `to_json()`
- `HealthStatus` enum: `Healthy`, `Degraded`, `Unhealthy`
- Configurable `failure_threshold()` for consecutive failure tracking
