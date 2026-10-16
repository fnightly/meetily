//! Shared HTTP client construction.
//!
//! Model downloads and provider calls used `reqwest::Client::new()`, which does
//! **not** read the operating system's proxy settings: `reqwest` 0.11 only does
//! so when the `system-proxy` feature is enabled, and it is not. On a machine
//! behind a proxy (a very common configuration for reaching huggingface.co),
//! requests therefore went direct and failed with a bare
//! "error trying to connect: tcp connect error" while every browser and
//! PowerShell command worked — a confusing failure mode.
//!
//! Everything that talks to the network should build its client here.

use reqwest::Client;

/// Environment variables checked, in order, for an explicit proxy.
const PROXY_ENV_VARS: &[&str] =
    &["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"];

/// Read the Windows "Internet Options" proxy from the registry.
///
/// Returns `None` when proxying is disabled, on non-Windows, or when the value
/// is unreadable. Deliberately a plain `Command` call to `reg.exe` rather than a
/// Win32 dependency, because this is a single read performed once per client.
/// Interpret a `reg query` value token, which may be decimal or `0x`-prefixed.
///
/// `ProxyEnable` is a `REG_DWORD` and comes back as `0x1`. `str::parse::<u32>()`
/// rejects the `0x` prefix, so parsing it naively returned `None`, the enable
/// check defaulted to "disabled", and system-proxy detection silently never
/// fired — the fix appeared to work while doing nothing.
///
/// Pure so both formats are covered by unit tests.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_reg_dword(token: &str) -> Option<u32> {
    let token = token.trim();

    if let Some(hex) = token.strip_prefix("0x").or_else(|| token.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).ok();
    }

    token.parse::<u32>().ok()
}

#[cfg(target_os = "windows")]
fn windows_system_proxy() -> Option<String> {
    use std::process::Command;

    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    let output = Command::new("reg")
        .args(["query", key, "/v", "ProxyEnable"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let enable_text = String::from_utf8_lossy(&output.stdout);
    let enabled = enable_text
        .lines()
        .find(|line| line.contains("ProxyEnable"))
        .and_then(|line| line.split_whitespace().last())
        .and_then(parse_reg_dword);

    if enabled != Some(1) {
        return None;
    }

    let output = Command::new("reg")
        .args(["query", key, "/v", "ProxyServer"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let server_text = String::from_utf8_lossy(&output.stdout);

    // The value is the last whitespace-separated field on the `ProxyServer` line,
    // e.g. "    ProxyServer    REG_SZ    127.0.0.1:10808".
    server_text
        .lines()
        .find(|line| line.contains("ProxyServer"))
        .and_then(|line| line.split_whitespace().last())
        .map(str::to_string)
        .filter(|value| !value.is_empty())
        .filter(|value| *value != "REG_SZ")
}

#[cfg(not(target_os = "windows"))]
fn windows_system_proxy() -> Option<String> {
    None
}

/// Extract a usable proxy URL from a Windows `ProxyServer` value.
///
/// That value has two shapes:
/// - `127.0.0.1:10808` — one proxy for every protocol;
/// - `http=127.0.0.1:8080;https=127.0.0.1:8080` — per-protocol mappings.
///
/// Scheme-less entries are given an `http://` prefix, which is what `reqwest`
/// requires (a bare `host:port` is rejected). Pure so it can be unit tested.
pub fn proxy_url_from_system_value(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let candidate = if trimmed.contains('=') {
        // Prefer the HTTPS mapping; fall back to the first entry present.
        let mut chosen: Option<&str> = None;
        for entry in trimmed.split(';') {
            let Some((scheme, value)) = entry.split_once('=') else {
                continue;
            };
            let scheme = scheme.trim();
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            if scheme.eq_ignore_ascii_case("https") {
                chosen = Some(value);
                break;
            }
            if chosen.is_none() {
                chosen = Some(value);
            }
        }
        chosen?
    } else {
        trimmed
    };

    let with_scheme = if candidate.contains("://") {
        candidate.to_string()
    } else {
        format!("http://{}", candidate)
    };

    Some(with_scheme)
}

/// Resolve the proxy to use, preferring explicit environment variables.
pub fn resolve_proxy_url() -> Option<String> {
    for name in PROXY_ENV_VARS {
        if let Ok(value) = std::env::var(name) {
            if let Some(url) = proxy_url_from_system_value(&value) {
                return Some(url);
            }
        }
    }

    windows_system_proxy().and_then(|value| proxy_url_from_system_value(&value))
}

/// Hosts that must never be sent through a proxy.
///
/// Loopback has to bypass it: the local Ollama endpoint, the bundled sidecar, and
/// any test running a throwaway HTTP server on 127.0.0.1 all break when their
/// traffic is handed to an external proxy.
fn loopback_bypass_list() -> String {
    let mut list = "localhost,127.0.0.1,::1".to_string();

    for name in ["NO_PROXY", "no_proxy"] {
        if let Ok(value) = std::env::var(name) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                list = format!("{list},{trimmed}");
                break;
            }
        }
    }

    list
}

/// Apply the resolved proxy to a client builder, keeping loopback direct.
///
/// Every proxy-aware client goes through this so the loopback exemption cannot
/// be forgotten at one call site — which is exactly what happened to the
/// Parakeet download client, whose loopback-hosted tests then failed.
pub fn apply_proxy(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    let Some(proxy_url) = resolve_proxy_url() else {
        return builder;
    };

    let proxy = match reqwest::Proxy::all(&proxy_url) {
        Ok(proxy) => proxy,
        Err(e) => {
            log::warn!(
                "Ignoring unusable proxy '{}' ({}); connecting directly",
                proxy_url,
                e
            );
            return builder;
        }
    };

    log::info!(
        "HTTP client using proxy {} (loopback stays direct)",
        proxy_url
    );

    builder.proxy(
        proxy.no_proxy(reqwest::NoProxy::from_string(&loopback_bypass_list())),
    )
}

/// A `reqwest` client that honours the environment/system proxy.
///
/// Falls back to a direct-connect client if the proxy URL is malformed, so a bad
/// setting degrades to the previous behaviour instead of breaking every request.
pub fn client_with_proxy() -> Client {
    let builder = Client::builder().user_agent(concat!("Meetily/", env!("CARGO_PKG_VERSION")));

    apply_proxy(builder).build().unwrap_or_else(|e| {
        log::warn!("Failed to build HTTP client ({e}); falling back to default");
        Client::new()
    })
}

/// A proxy-aware client with an overall request timeout.
///
/// Used by the provider connectivity tests, which previously built a client with
/// a timeout but no proxy awareness.
pub fn client_with_proxy_and_timeout(timeout: std::time::Duration) -> Client {
    let builder = Client::builder()
        .user_agent(concat!("Meetily/", env!("CARGO_PKG_VERSION")))
        .timeout(timeout);

    apply_proxy(builder).build().unwrap_or_else(|e| {
        log::warn!("Failed to build HTTP client ({e}); falling back to default");
        Client::new()
    })
}

/// A blocking client that honours the proxy, for the few synchronous call sites.
///
/// Prefer [`client_with_proxy`]: a blocking call made from a Tauri command runs
/// on the main thread and freezes the UI for its whole duration.
pub fn blocking_client_with_proxy() -> reqwest::blocking::Client {
    blocking_client_with_proxy_and_timeout(None)
}

/// Blocking client with an optional overall request timeout.
///
/// `reqwest::blocking::Client` has no `.timeout()` setter, so the timeout must be
/// supplied when the builder is constructed.
pub fn blocking_client_with_proxy_and_timeout(
    timeout: Option<std::time::Duration>,
) -> reqwest::blocking::Client {
    let mut builder = reqwest::blocking::Client::builder()
        .user_agent(concat!("Meetily/", env!("CARGO_PKG_VERSION")));

    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }

    if let Some(proxy_url) = resolve_proxy_url() {
        match reqwest::Proxy::all(&proxy_url) {
            Ok(proxy) => {
                log::info!(
                    "Blocking HTTP client using proxy {} (loopback stays direct)",
                    proxy_url
                );
                // Loopback bypass matters here too: the Ollama model listing runs
                // through this client against localhost.
                builder = builder.proxy(
                    proxy.no_proxy(reqwest::NoProxy::from_string(&loopback_bypass_list())),
                );
            }
            Err(e) => {
                log::warn!(
                    "Ignoring unusable proxy '{}' ({}); connecting directly",
                    proxy_url,
                    e
                );
            }
        }
    }

    builder.build().unwrap_or_else(|e| {
        log::warn!("Failed to build blocking HTTP client ({e}); falling back to default");
        reqwest::blocking::Client::new()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_host_port_gets_http_scheme() {
        assert_eq!(
            proxy_url_from_system_value("127.0.0.1:10808"),
            Some("http://127.0.0.1:10808".to_string())
        );
    }

    #[test]
    fn existing_scheme_is_preserved() {
        assert_eq!(
            proxy_url_from_system_value("socks5://127.0.0.1:1080"),
            Some("socks5://127.0.0.1:1080".to_string())
        );
        assert_eq!(
            proxy_url_from_system_value("http://proxy.local:3128"),
            Some("http://proxy.local:3128".to_string())
        );
    }

    #[test]
    fn per_protocol_mapping_prefers_https() {
        let value = "http=127.0.0.1:8080;https=127.0.0.1:8443";
        assert_eq!(
            proxy_url_from_system_value(value),
            Some("http://127.0.0.1:8443".to_string())
        );
    }

    #[test]
    fn per_protocol_mapping_falls_back_to_first_entry() {
        let value = "http=127.0.0.1:8080;ftp=127.0.0.1:2121";
        assert_eq!(
            proxy_url_from_system_value(value),
            Some("http://127.0.0.1:8080".to_string())
        );
    }

    #[test]
    fn blank_values_are_rejected() {
        assert_eq!(proxy_url_from_system_value(""), None);
        assert_eq!(proxy_url_from_system_value("   "), None);
        assert_eq!(proxy_url_from_system_value("http=;https="), None);
    }

    #[test]
    fn whitespace_is_trimmed() {
        assert_eq!(
            proxy_url_from_system_value("  127.0.0.1:10808  "),
            Some("http://127.0.0.1:10808".to_string())
        );
    }

    #[test]
    fn building_a_client_never_panics() {
        // The real value of this test is that client construction succeeds on a
        // machine with a system proxy configured (as the reporter's is).
        let _client = client_with_proxy();
    }

    #[test]
    fn resolution_does_not_panic() {
        let _ = resolve_proxy_url();
    }

    #[test]
    fn reg_dword_parses_hex_encoding() {
        // `reg query` reports REG_DWORD as hex; `parse::<u32>()` rejects this,
        // which is exactly how system-proxy detection silently failed.
        assert_eq!(parse_reg_dword("0x1"), Some(1));
        assert_eq!(parse_reg_dword("0X1"), Some(1));
        assert_eq!(parse_reg_dword("0x0"), Some(0));
        assert_eq!(parse_reg_dword("0xF"), Some(15));
    }

    #[test]
    fn reg_dword_parses_decimal_encoding() {
        assert_eq!(parse_reg_dword("1"), Some(1));
        assert_eq!(parse_reg_dword("0"), Some(0));
    }

    #[test]
    fn reg_dword_ignores_surrounding_whitespace() {
        assert_eq!(parse_reg_dword("  0x1  "), Some(1));
    }

    #[test]
    fn reg_dword_rejects_non_numeric_tokens() {
        assert_eq!(parse_reg_dword("REG_DWORD"), None);
        assert_eq!(parse_reg_dword(""), None);
        assert_eq!(parse_reg_dword("0xZZ"), None);
    }

    #[test]
    fn bypass_list_always_contains_loopback() {
        // The list is what `reqwest::NoProxy` matches against, so it must name
        // every loopback spelling a local service might use.
        let list = loopback_bypass_list();
        for entry in ["localhost", "127.0.0.1", "::1"] {
            assert!(list.contains(entry), "bypass list is missing {entry}");
        }
    }

    #[test]
    fn bypass_list_keeps_loopback_even_when_no_proxy_is_set() {
        // Loopback exemption must not depend on the user having configured
        // NO_PROXY themselves.
        let list = loopback_bypass_list();
        assert!(list.starts_with("localhost,127.0.0.1,::1"));
    }

    /// End-to-end check that the configured client can actually reach the model
    /// host through the proxy.
    ///
    /// Network-dependent, so it is ignored by default. Run it on a machine that
    /// requires a proxy to confirm the fix is real rather than merely
    /// type-correct:
    ///
    /// ```text
    /// cargo test -p meetily --lib http_client -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "performs real network I/O; run explicitly to verify proxy handling"]
    async fn reaches_model_host_through_configured_proxy() {
        let proxy = resolve_proxy_url();
        println!("resolved proxy: {proxy:?}");

        let client = client_with_proxy_and_timeout(std::time::Duration::from_secs(30));
        let url = crate::config::whisper_model_url("ggml-tiny-q5_1.bin");
        println!("probing {url}");

        // A HEAD avoids downloading the model while still proving connectivity.
        let response = client.head(&url).send().await;

        match response {
            Ok(response) => {
                println!("status: {}", response.status());
                assert!(
                    response.status().is_success(),
                    "expected a success status, got {}",
                    response.status()
                );
            }
            Err(e) => panic!("request through proxy failed: {e}"),
        }
    }
}
