# rs-healthcheck-client

[![CI](https://github.com/philiprehberger/rs-healthcheck-client/actions/workflows/ci.yml/badge.svg)](https://github.com/philiprehberger/rs-healthcheck-client/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/philiprehberger-healthcheck-client.svg)](https://crates.io/crates/philiprehberger-healthcheck-client)
[![Last updated](https://img.shields.io/github/last-commit/philiprehberger/rs-healthcheck-client)](https://github.com/philiprehberger/rs-healthcheck-client/commits/main)

HTTP health check client for monitoring service dependencies with configurable thresholds

## Installation

```toml
[dependencies]
philiprehberger-healthcheck-client = "0.3.0"
```

## Usage

```rust
use philiprehberger_healthcheck_client::HealthChecker;

#[tokio::main]
async fn main() {
    let mut checker = HealthChecker::new();
    checker
        .add_http("api", "http://localhost:8080/health")
        .add_tcp("database", "127.0.0.1", 5432)
        .add_tcp("redis", "127.0.0.1", 6379);

    let report = checker.check_all().await;
    println!("{}", report.summary());
}
```

### Custom Timeouts and Expected Status

```rust
use philiprehberger_healthcheck_client::HealthChecker;

let mut checker = HealthChecker::new();
checker
    .add_http_with("api", "http://localhost:8080/health", 200, 3000)
    .add_tcp_with("db", "127.0.0.1", 5432, 2000);
```

### HTTP Method (HEAD vs GET)

```rust
use philiprehberger_healthcheck_client::{HealthChecker, HttpMethod};

let mut checker = HealthChecker::new();
checker.add_http_with_method("api", "http://localhost:8080/health", HttpMethod::Head);
```

### Status Code Range

```rust
use philiprehberger_healthcheck_client::HealthChecker;

let mut checker = HealthChecker::new();
// Accept any 2xx response
checker.add_http_status_range("api", "http://localhost:8080/health", 200, 299);
```

### Latency Aggregates

```rust
let report = checker.check_all().await;
println!("p50: {:?}ms", report.latency_p50());
println!("p95: {:?}ms", report.latency_p95());
```

### Custom Checks

```rust
use philiprehberger_healthcheck_client::{HealthChecker, CheckResult, HealthStatus};

let mut checker = HealthChecker::new();
checker.add_custom("disk-space", || {
    CheckResult {
        name: String::new(),
        status: HealthStatus::Healthy,
        latency_ms: 0,
        message: Some("80% free".to_string()),
        timestamp: 0,
    }
});
```

### Check a Single Service

```rust
if let Some(result) = checker.check_one("api").await {
    println!("{}: {}", result.name, result.status);
}
```

### JSON Output for /health Endpoints

```rust
let report = checker.check_all().await;
let json = report.to_json();
// Serve `json` as the response body for your /health endpoint
```

## API

| Type / Method | Description |
|---|---|
| `HealthStatus` | Enum: `Healthy`, `Degraded`, `Unhealthy` |
| `CheckResult` | Result of a single check (name, status, latency, message, timestamp) |
| `Check` | Check definition enum: `Http`, `Tcp`, `Custom` |
| `HttpMethod` | Enum: `Get`, `Head` |
| `StatusMatch` | Enum: `Exact(code)`, `Range(min, max)` |
| `HealthChecker::new()` | Create a new checker with no checks |
| `.add_http(name, url)` | Add HTTP `GET` check (expects 200, 5s timeout) |
| `.add_http_with(name, url, status, timeout_ms)` | Add HTTP `GET` check with custom settings |
| `.add_http_with_method(name, url, method)` | Add HTTP check with the given `HttpMethod` |
| `.add_http_status_range(name, url, min, max)` | Add HTTP check accepting any code in `[min, max]` |
| `.add_tcp(name, host, port)` | Add TCP check (5s timeout) |
| `.add_tcp_with(name, host, port, timeout_ms)` | Add TCP check with custom timeout |
| `.add_custom(name, fn)` | Add a custom check function |
| `.failure_threshold(n)` | Set consecutive failure threshold |
| `.check_all()` | Run all checks in parallel, return `HealthReport` |
| `.check_one(name)` | Run a single check by name |
| `HealthReport.overall` | Overall status derived from all checks |
| `HealthReport.is_healthy()` | Returns `true` if overall status is Healthy |
| `HealthReport.is_degraded()` | Returns `true` if overall status is Degraded |
| `HealthReport.failed_checks()` | Returns only `Unhealthy` check results |
| `HealthReport.unhealthy_checks()` | Returns non-healthy check results |
| `HealthReport.healthy_checks()` | Returns only `Healthy` check results |
| `HealthReport.latency_p50()` | Median latency across checks (ms), or `None` if empty |
| `HealthReport.latency_p95()` | 95th percentile latency (ms), or `None` if empty |
| `HealthReport.to_json()` | Serialize report to JSON |
| `HealthReport.summary()` | Human-readable summary string |

## Development

```bash
cargo test
cargo clippy -- -D warnings
```

## Support

If you find this project useful:

⭐ [Star the repo](https://github.com/philiprehberger/rs-healthcheck-client)

🐛 [Report issues](https://github.com/philiprehberger/rs-healthcheck-client/issues?q=is%3Aissue+is%3Aopen+label%3Abug)

💡 [Suggest features](https://github.com/philiprehberger/rs-healthcheck-client/issues?q=is%3Aissue+is%3Aopen+label%3Aenhancement)

❤️ [Sponsor development](https://github.com/sponsors/philiprehberger)

🌐 [All Open Source Projects](https://philiprehberger.com/open-source-packages)

💻 [GitHub Profile](https://github.com/philiprehberger)

🔗 [LinkedIn Profile](https://www.linkedin.com/in/philiprehberger)

## License

[MIT](LICENSE)
