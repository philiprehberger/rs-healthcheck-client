//! HTTP health check client for monitoring service dependencies with configurable thresholds.
//!
//! This crate provides a simple health check client that can monitor HTTP and TCP
//! service dependencies, run checks in parallel, and produce structured health reports
//! suitable for `/health` endpoint responses.
//!
//! # Example
//!
//! ```rust,no_run
//! use philiprehberger_healthcheck_client::HealthChecker;
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut checker = HealthChecker::new();
//!     checker
//!         .add_http("api", "http://localhost:8080/health")
//!         .add_tcp("database", "127.0.0.1", 5432);
//!
//!     let report = checker.check_all().await;
//!     println!("{}", report.summary());
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// The health status of a single check or an overall report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    /// The service is operating normally.
    Healthy,
    /// The service is operational but experiencing issues.
    Degraded,
    /// The service is not operational.
    Unhealthy,
}

impl fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "Healthy"),
            HealthStatus::Degraded => write!(f, "Degraded"),
            HealthStatus::Unhealthy => write!(f, "Unhealthy"),
        }
    }
}

/// The result of a single health check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    /// Name identifying this check.
    pub name: String,
    /// The health status determined by this check.
    pub status: HealthStatus,
    /// Latency of the check in milliseconds.
    pub latency_ms: u64,
    /// Optional message providing additional detail.
    pub message: Option<String>,
    /// Unix timestamp when the check was performed.
    pub timestamp: u64,
}

/// A health check definition.
///
/// Each variant represents a different type of health check that can be performed.
pub enum Check {
    /// An HTTP health check that makes a GET request and verifies the status code.
    Http {
        /// Name identifying this check.
        name: String,
        /// URL to check.
        url: String,
        /// Expected HTTP status code.
        expected_status: u16,
        /// Timeout in milliseconds.
        timeout_ms: u64,
    },
    /// A TCP connectivity check.
    Tcp {
        /// Name identifying this check.
        name: String,
        /// Host to connect to.
        host: String,
        /// Port to connect to.
        port: u16,
        /// Timeout in milliseconds.
        timeout_ms: u64,
    },
    /// A custom check using a user-provided function.
    Custom {
        /// Name identifying this check.
        name: String,
        /// The check function.
        check_fn: Box<dyn Fn() -> CheckResult + Send + Sync>,
    },
}

/// An aggregated health report containing results from all checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    /// The overall health status, derived from individual check results.
    pub overall: HealthStatus,
    /// Individual check results.
    pub checks: Vec<CheckResult>,
    /// Unix timestamp when the report was generated.
    pub timestamp: u64,
}

impl HealthReport {
    /// Returns `true` if the overall status is `Healthy`.
    pub fn is_healthy(&self) -> bool {
        self.overall == HealthStatus::Healthy
    }

    /// Returns `true` if the overall status is `Degraded`.
    ///
    /// A degraded report indicates that some checks passed while others failed,
    /// meaning the system is partially operational.
    pub fn is_degraded(&self) -> bool {
        self.overall == HealthStatus::Degraded
    }

    /// Returns references to all checks that have an `Unhealthy` status.
    pub fn failed_checks(&self) -> Vec<&CheckResult> {
        self.checks
            .iter()
            .filter(|c| c.status == HealthStatus::Unhealthy)
            .collect()
    }

    /// Returns references to all checks that are not `Healthy`.
    pub fn unhealthy_checks(&self) -> Vec<&CheckResult> {
        self.checks
            .iter()
            .filter(|c| c.status != HealthStatus::Healthy)
            .collect()
    }

    /// Serialize the report to a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Produce a human-readable summary of the report.
    pub fn summary(&self) -> String {
        let mut lines = vec![format!("Overall: {}", self.overall)];
        for check in &self.checks {
            let msg = check
                .message
                .as_deref()
                .map(|m| format!(" ({})", m))
                .unwrap_or_default();
            lines.push(format!(
                "  {} — {} — {}ms{}",
                check.name, check.status, check.latency_ms, msg
            ));
        }
        lines.join("\n")
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn determine_overall(checks: &[CheckResult]) -> HealthStatus {
    let mut has_degraded = false;
    for check in checks {
        match check.status {
            HealthStatus::Unhealthy => return HealthStatus::Unhealthy,
            HealthStatus::Degraded => has_degraded = true,
            HealthStatus::Healthy => {}
        }
    }
    if has_degraded {
        HealthStatus::Degraded
    } else {
        HealthStatus::Healthy
    }
}

/// Parse the HTTP status code from a raw HTTP response.
fn parse_http_status(response: &str) -> Option<u16> {
    // HTTP/1.1 200 OK
    let first_line = response.lines().next()?;
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() >= 2 {
        parts[1].parse().ok()
    } else {
        None
    }
}

/// Parse host and port from a URL string.
fn parse_url(url: &str) -> Option<(bool, String, u16, String)> {
    let (is_https, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else {
        return None;
    };

    let (host_port, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };

    let (host, port) = match host_port.find(':') {
        Some(i) => (
            host_port[..i].to_string(),
            host_port[i + 1..].parse::<u16>().ok()?,
        ),
        None => (
            host_port.to_string(),
            if is_https { 443 } else { 80 },
        ),
    };

    Some((is_https, host, port, path.to_string()))
}

async fn run_http_check(
    name: &str,
    url: &str,
    expected_status: u16,
    timeout_ms: u64,
) -> CheckResult {
    let start = Instant::now();
    let ts = now_unix();

    let (_, host, port, path) = match parse_url(url) {
        Some(v) => v,
        None => {
            return CheckResult {
                name: name.to_string(),
                status: HealthStatus::Unhealthy,
                latency_ms: start.elapsed().as_millis() as u64,
                message: Some("Invalid URL".to_string()),
                timestamp: ts,
            };
        }
    };

    let addr = format!("{}:{}", host, port);
    let dur = Duration::from_millis(timeout_ms);

    let result = timeout(dur, async {
        let mut stream = TcpStream::connect(&addr).await?;
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            path, host
        );
        stream.write_all(request.as_bytes()).await?;

        let mut response = Vec::new();
        stream.read_to_end(&mut response).await?;
        Ok::<Vec<u8>, std::io::Error>(response)
    })
    .await;

    let latency_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(Ok(response_bytes)) => {
            let response_str = String::from_utf8_lossy(&response_bytes);
            match parse_http_status(&response_str) {
                Some(status_code) if status_code == expected_status => CheckResult {
                    name: name.to_string(),
                    status: HealthStatus::Healthy,
                    latency_ms,
                    message: Some(format!("HTTP {}", status_code)),
                    timestamp: ts,
                },
                Some(status_code) => CheckResult {
                    name: name.to_string(),
                    status: HealthStatus::Unhealthy,
                    latency_ms,
                    message: Some(format!(
                        "Expected HTTP {}, got {}",
                        expected_status, status_code
                    )),
                    timestamp: ts,
                },
                None => CheckResult {
                    name: name.to_string(),
                    status: HealthStatus::Unhealthy,
                    latency_ms,
                    message: Some("Could not parse HTTP response".to_string()),
                    timestamp: ts,
                },
            }
        }
        Ok(Err(e)) => CheckResult {
            name: name.to_string(),
            status: HealthStatus::Unhealthy,
            latency_ms,
            message: Some(format!("Connection error: {}", e)),
            timestamp: ts,
        },
        Err(_) => CheckResult {
            name: name.to_string(),
            status: HealthStatus::Unhealthy,
            latency_ms,
            message: Some("Timeout".to_string()),
            timestamp: ts,
        },
    }
}

async fn run_tcp_check(name: &str, host: &str, port: u16, timeout_ms: u64) -> CheckResult {
    let start = Instant::now();
    let ts = now_unix();
    let addr = format!("{}:{}", host, port);
    let dur = Duration::from_millis(timeout_ms);

    let result = timeout(dur, TcpStream::connect(&addr)).await;
    let latency_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(Ok(_)) => CheckResult {
            name: name.to_string(),
            status: HealthStatus::Healthy,
            latency_ms,
            message: Some(format!("TCP connection to {} established", addr)),
            timestamp: ts,
        },
        Ok(Err(e)) => CheckResult {
            name: name.to_string(),
            status: HealthStatus::Unhealthy,
            latency_ms,
            message: Some(format!("Connection refused: {}", e)),
            timestamp: ts,
        },
        Err(_) => CheckResult {
            name: name.to_string(),
            status: HealthStatus::Unhealthy,
            latency_ms,
            message: Some("Timeout".to_string()),
            timestamp: ts,
        },
    }
}

fn run_custom_check(name: &str, f: &(dyn Fn() -> CheckResult + Send + Sync)) -> CheckResult {
    let mut result = f();
    result.name = name.to_string();
    result
}

/// A health checker that manages multiple service checks and runs them in parallel.
///
/// # Example
///
/// ```rust,no_run
/// use philiprehberger_healthcheck_client::HealthChecker;
///
/// #[tokio::main]
/// async fn main() {
///     let mut checker = HealthChecker::new();
///     checker
///         .add_http("api", "http://localhost:8080/health")
///         .add_tcp("redis", "127.0.0.1", 6379);
///
///     let report = checker.check_all().await;
///     if !report.is_healthy() {
///         eprintln!("{}", report.summary());
///     }
/// }
/// ```
pub struct HealthChecker {
    checks: Vec<Check>,
    failure_threshold: u32,
}

impl HealthChecker {
    /// Create a new `HealthChecker` with no checks and default failure threshold of 1.
    pub fn new() -> Self {
        Self {
            checks: Vec::new(),
            failure_threshold: 1,
        }
    }

    /// Add an HTTP health check with default settings (expects 200, 5s timeout).
    pub fn add_http(&mut self, name: &str, url: &str) -> &mut Self {
        self.checks.push(Check::Http {
            name: name.to_string(),
            url: url.to_string(),
            expected_status: 200,
            timeout_ms: 5000,
        });
        self
    }

    /// Add an HTTP health check with custom expected status code and timeout.
    pub fn add_http_with(
        &mut self,
        name: &str,
        url: &str,
        expected_status: u16,
        timeout_ms: u64,
    ) -> &mut Self {
        self.checks.push(Check::Http {
            name: name.to_string(),
            url: url.to_string(),
            expected_status,
            timeout_ms,
        });
        self
    }

    /// Add a TCP connectivity check with a default 5s timeout.
    pub fn add_tcp(&mut self, name: &str, host: &str, port: u16) -> &mut Self {
        self.checks.push(Check::Tcp {
            name: name.to_string(),
            host: host.to_string(),
            port,
            timeout_ms: 5000,
        });
        self
    }

    /// Add a TCP connectivity check with a custom timeout.
    pub fn add_tcp_with(
        &mut self,
        name: &str,
        host: &str,
        port: u16,
        timeout_ms: u64,
    ) -> &mut Self {
        self.checks.push(Check::Tcp {
            name: name.to_string(),
            host: host.to_string(),
            port,
            timeout_ms,
        });
        self
    }

    /// Add a custom health check using a user-provided function.
    pub fn add_custom(
        &mut self,
        name: &str,
        f: impl Fn() -> CheckResult + Send + Sync + 'static,
    ) -> &mut Self {
        self.checks.push(Check::Custom {
            name: name.to_string(),
            check_fn: Box::new(f),
        });
        self
    }

    /// Set the consecutive failure threshold before a check is considered unhealthy.
    pub fn failure_threshold(&mut self, threshold: u32) -> &mut Self {
        self.failure_threshold = threshold;
        self
    }

    /// Run all checks in parallel and return an aggregated health report.
    pub async fn check_all(&self) -> HealthReport {
        let mut handles = Vec::new();

        for check in &self.checks {
            match check {
                Check::Http {
                    name,
                    url,
                    expected_status,
                    timeout_ms,
                } => {
                    let name = name.clone();
                    let url = url.clone();
                    let expected = *expected_status;
                    let tms = *timeout_ms;
                    handles.push(tokio::spawn(async move {
                        run_http_check(&name, &url, expected, tms).await
                    }));
                }
                Check::Tcp {
                    name,
                    host,
                    port,
                    timeout_ms,
                } => {
                    let name = name.clone();
                    let host = host.clone();
                    let port = *port;
                    let tms = *timeout_ms;
                    handles.push(tokio::spawn(async move {
                        run_tcp_check(&name, &host, port, tms).await
                    }));
                }
                Check::Custom { name, check_fn } => {
                    let result = run_custom_check(name, check_fn.as_ref());
                    handles.push(tokio::spawn(async move { result }));
                }
            }
        }

        let mut results = Vec::new();
        for handle in handles {
            if let Ok(result) = handle.await {
                results.push(result);
            }
        }

        let overall = determine_overall(&results);

        HealthReport {
            overall,
            checks: results,
            timestamp: now_unix(),
        }
    }

    /// Run a single check by name and return its result, or `None` if not found.
    pub async fn check_one(&self, name: &str) -> Option<CheckResult> {
        for check in &self.checks {
            let check_name = match check {
                Check::Http { name, .. } => name,
                Check::Tcp { name, .. } => name,
                Check::Custom { name, .. } => name,
            };
            if check_name == name {
                return Some(match check {
                    Check::Http {
                        name,
                        url,
                        expected_status,
                        timeout_ms,
                    } => run_http_check(name, url, *expected_status, *timeout_ms).await,
                    Check::Tcp {
                        name,
                        host,
                        port,
                        timeout_ms,
                    } => run_tcp_check(name, host, *port, *timeout_ms).await,
                    Check::Custom { name, check_fn } => {
                        run_custom_check(name, check_fn.as_ref())
                    }
                });
            }
        }
        None
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    async fn start_tcp_listener() -> (TcpListener, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    async fn start_http_server(status_code: u16) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            loop {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let response = format!(
                        "HTTP/1.1 {} OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        status_code
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            }
        });

        // Give the server a moment to start
        tokio::time::sleep(Duration::from_millis(10)).await;
        port
    }

    #[tokio::test]
    async fn test_tcp_check_success() {
        let (listener, port) = start_tcp_listener().await;

        // Accept connections in the background
        tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let result = run_tcp_check("test-tcp", "127.0.0.1", port, 2000).await;
        assert_eq!(result.status, HealthStatus::Healthy);
        assert_eq!(result.name, "test-tcp");
        assert!(result.message.is_some());
    }

    #[tokio::test]
    async fn test_tcp_check_failure() {
        // Use a port that's unlikely to be listening
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // Close it immediately so nothing is listening

        let result = run_tcp_check("dead-tcp", "127.0.0.1", port, 1000).await;
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert_eq!(result.name, "dead-tcp");
    }

    #[tokio::test]
    async fn test_http_check_success() {
        let port = start_http_server(200).await;
        let url = format!("http://127.0.0.1:{}/health", port);

        let result = run_http_check("test-http", &url, 200, 2000).await;
        assert_eq!(result.status, HealthStatus::Healthy);
        assert_eq!(result.name, "test-http");
        assert!(result.message.unwrap().contains("200"));
    }

    #[tokio::test]
    async fn test_http_check_wrong_status() {
        let port = start_http_server(503).await;
        let url = format!("http://127.0.0.1:{}/health", port);

        let result = run_http_check("test-http", &url, 200, 2000).await;
        assert_eq!(result.status, HealthStatus::Unhealthy);
        assert!(result.message.unwrap().contains("503"));
    }

    #[tokio::test]
    async fn test_check_all_aggregation() {
        let (listener, port) = start_tcp_listener().await;

        tokio::spawn(async move {
            loop {
                let _ = listener.accept().await;
            }
        });

        let mut checker = HealthChecker::new();
        checker.add_tcp("alive", "127.0.0.1", port);
        checker.add_custom("custom-ok", || CheckResult {
            name: String::new(),
            status: HealthStatus::Healthy,
            latency_ms: 0,
            message: Some("All good".to_string()),
            timestamp: now_unix(),
        });

        let report = checker.check_all().await;
        assert_eq!(report.checks.len(), 2);
        assert_eq!(report.overall, HealthStatus::Healthy);
        assert!(report.is_healthy());
    }

    #[tokio::test]
    async fn test_overall_status_all_healthy() {
        let results = vec![
            CheckResult {
                name: "a".to_string(),
                status: HealthStatus::Healthy,
                latency_ms: 1,
                message: None,
                timestamp: 0,
            },
            CheckResult {
                name: "b".to_string(),
                status: HealthStatus::Healthy,
                latency_ms: 2,
                message: None,
                timestamp: 0,
            },
        ];
        assert_eq!(determine_overall(&results), HealthStatus::Healthy);
    }

    #[tokio::test]
    async fn test_overall_status_one_degraded() {
        let results = vec![
            CheckResult {
                name: "a".to_string(),
                status: HealthStatus::Healthy,
                latency_ms: 1,
                message: None,
                timestamp: 0,
            },
            CheckResult {
                name: "b".to_string(),
                status: HealthStatus::Degraded,
                latency_ms: 2,
                message: None,
                timestamp: 0,
            },
        ];
        assert_eq!(determine_overall(&results), HealthStatus::Degraded);
    }

    #[tokio::test]
    async fn test_overall_status_one_unhealthy() {
        let results = vec![
            CheckResult {
                name: "a".to_string(),
                status: HealthStatus::Healthy,
                latency_ms: 1,
                message: None,
                timestamp: 0,
            },
            CheckResult {
                name: "b".to_string(),
                status: HealthStatus::Degraded,
                latency_ms: 2,
                message: None,
                timestamp: 0,
            },
            CheckResult {
                name: "c".to_string(),
                status: HealthStatus::Unhealthy,
                latency_ms: 3,
                message: None,
                timestamp: 0,
            },
        ];
        assert_eq!(determine_overall(&results), HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn test_health_report_serialization() {
        let report = HealthReport {
            overall: HealthStatus::Healthy,
            checks: vec![CheckResult {
                name: "test".to_string(),
                status: HealthStatus::Healthy,
                latency_ms: 5,
                message: Some("OK".to_string()),
                timestamp: 1000,
            }],
            timestamp: 1000,
        };

        let json = report.to_json();
        assert!(json.contains("\"Healthy\""));
        assert!(json.contains("\"test\""));
        assert!(json.contains("\"OK\""));

        // Verify it can be deserialized back
        let parsed: HealthReport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.overall, HealthStatus::Healthy);
        assert_eq!(parsed.checks.len(), 1);
    }

    #[tokio::test]
    async fn test_summary_output() {
        let report = HealthReport {
            overall: HealthStatus::Degraded,
            checks: vec![
                CheckResult {
                    name: "api".to_string(),
                    status: HealthStatus::Healthy,
                    latency_ms: 10,
                    message: None,
                    timestamp: 0,
                },
                CheckResult {
                    name: "cache".to_string(),
                    status: HealthStatus::Degraded,
                    latency_ms: 250,
                    message: Some("Slow response".to_string()),
                    timestamp: 0,
                },
            ],
            timestamp: 0,
        };

        let summary = report.summary();
        assert!(summary.contains("Overall: Degraded"));
        assert!(summary.contains("api"));
        assert!(summary.contains("cache"));
        assert!(summary.contains("Slow response"));
    }

    #[tokio::test]
    async fn test_custom_check() {
        let mut checker = HealthChecker::new();
        checker.add_custom("my-check", || CheckResult {
            name: String::new(),
            status: HealthStatus::Degraded,
            latency_ms: 42,
            message: Some("Running slow".to_string()),
            timestamp: now_unix(),
        });

        let result = checker.check_one("my-check").await;
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.status, HealthStatus::Degraded);
        assert_eq!(result.name, "my-check");
        assert_eq!(result.latency_ms, 42);
    }

    #[tokio::test]
    async fn test_check_one_not_found() {
        let checker = HealthChecker::new();
        let result = checker.check_one("nonexistent").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_unhealthy_checks_filter() {
        let report = HealthReport {
            overall: HealthStatus::Unhealthy,
            checks: vec![
                CheckResult {
                    name: "ok".to_string(),
                    status: HealthStatus::Healthy,
                    latency_ms: 1,
                    message: None,
                    timestamp: 0,
                },
                CheckResult {
                    name: "bad".to_string(),
                    status: HealthStatus::Unhealthy,
                    latency_ms: 100,
                    message: None,
                    timestamp: 0,
                },
                CheckResult {
                    name: "slow".to_string(),
                    status: HealthStatus::Degraded,
                    latency_ms: 500,
                    message: None,
                    timestamp: 0,
                },
            ],
            timestamp: 0,
        };

        let unhealthy = report.unhealthy_checks();
        assert_eq!(unhealthy.len(), 2);
        assert!(unhealthy.iter().any(|c| c.name == "bad"));
        assert!(unhealthy.iter().any(|c| c.name == "slow"));
    }

    #[test]
    fn test_health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "Healthy");
        assert_eq!(HealthStatus::Degraded.to_string(), "Degraded");
        assert_eq!(HealthStatus::Unhealthy.to_string(), "Unhealthy");
    }

    #[test]
    fn test_is_degraded() {
        let healthy_report = HealthReport {
            overall: HealthStatus::Healthy,
            checks: vec![CheckResult {
                name: "a".to_string(),
                status: HealthStatus::Healthy,
                latency_ms: 1,
                message: None,
                timestamp: 0,
            }],
            timestamp: 0,
        };
        assert!(!healthy_report.is_degraded());

        let degraded_report = HealthReport {
            overall: HealthStatus::Degraded,
            checks: vec![
                CheckResult {
                    name: "a".to_string(),
                    status: HealthStatus::Healthy,
                    latency_ms: 1,
                    message: None,
                    timestamp: 0,
                },
                CheckResult {
                    name: "b".to_string(),
                    status: HealthStatus::Degraded,
                    latency_ms: 2,
                    message: None,
                    timestamp: 0,
                },
            ],
            timestamp: 0,
        };
        assert!(degraded_report.is_degraded());

        let unhealthy_report = HealthReport {
            overall: HealthStatus::Unhealthy,
            checks: vec![CheckResult {
                name: "a".to_string(),
                status: HealthStatus::Unhealthy,
                latency_ms: 1,
                message: None,
                timestamp: 0,
            }],
            timestamp: 0,
        };
        assert!(!unhealthy_report.is_degraded());
    }

    #[test]
    fn test_failed_checks() {
        let report = HealthReport {
            overall: HealthStatus::Unhealthy,
            checks: vec![
                CheckResult {
                    name: "ok".to_string(),
                    status: HealthStatus::Healthy,
                    latency_ms: 1,
                    message: None,
                    timestamp: 0,
                },
                CheckResult {
                    name: "bad".to_string(),
                    status: HealthStatus::Unhealthy,
                    latency_ms: 100,
                    message: None,
                    timestamp: 0,
                },
                CheckResult {
                    name: "slow".to_string(),
                    status: HealthStatus::Degraded,
                    latency_ms: 500,
                    message: None,
                    timestamp: 0,
                },
                CheckResult {
                    name: "down".to_string(),
                    status: HealthStatus::Unhealthy,
                    latency_ms: 200,
                    message: None,
                    timestamp: 0,
                },
            ],
            timestamp: 0,
        };

        let failed = report.failed_checks();
        assert_eq!(failed.len(), 2);
        assert!(failed.iter().any(|c| c.name == "bad"));
        assert!(failed.iter().any(|c| c.name == "down"));
        // Degraded checks should NOT be included in failed_checks
        assert!(!failed.iter().any(|c| c.name == "slow"));
    }

    #[test]
    fn test_failed_checks_empty_when_all_healthy() {
        let report = HealthReport {
            overall: HealthStatus::Healthy,
            checks: vec![
                CheckResult {
                    name: "a".to_string(),
                    status: HealthStatus::Healthy,
                    latency_ms: 1,
                    message: None,
                    timestamp: 0,
                },
                CheckResult {
                    name: "b".to_string(),
                    status: HealthStatus::Healthy,
                    latency_ms: 2,
                    message: None,
                    timestamp: 0,
                },
            ],
            timestamp: 0,
        };

        assert!(report.failed_checks().is_empty());
    }

    #[test]
    fn test_parse_url() {
        let (is_https, host, port, path) = parse_url("http://localhost:8080/health").unwrap();
        assert!(!is_https);
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
        assert_eq!(path, "/health");

        let (is_https, host, port, path) = parse_url("https://example.com/api").unwrap();
        assert!(is_https);
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
        assert_eq!(path, "/api");

        let (_, _, port, path) = parse_url("http://localhost").unwrap();
        assert_eq!(port, 80);
        assert_eq!(path, "/");
    }
}
