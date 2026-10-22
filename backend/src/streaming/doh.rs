//! DNS-over-HTTPS 解析器 / DNS-over-HTTPS resolver
//!
//! 部分网络（尤其是运营商级别的过滤）会污染 `stripchat.com` 的 DNS 解析结果，
//! 返回一个无法连接的占位 IP，表现为请求一直卡到超时。
//! 本模块通过 DoH（Cloudflare / Google）解析域名，绕开本地 DNS，并在失败时回退到系统解析。
//!
//! Some networks (especially ISP-level filtering) poison the DNS answer for `stripchat.com`
//! and return an unreachable placeholder IP, which makes every request hang until it times out.
//! This module resolves hostnames through DoH (Cloudflare / Google) to bypass the local
//! resolver, and falls back to the system resolver when DoH is unavailable.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// DoH 结果缓存时长 / How long a DoH answer is cached
const CACHE_TTL: Duration = Duration::from_secs(300);
/// 单次 DoH 查询超时 / Timeout for a single DoH query
const DOH_TIMEOUT: Duration = Duration::from_secs(5);

/// 缓存条目 / Cache entry
struct CacheEntry {
    addrs: Vec<SocketAddr>,
    expires: Instant,
}

/// 进程级共享缓存，客户端频繁重建时无需重复查询 DoH。
/// Process-wide shared cache so repeatedly rebuilt clients do not re-query DoH.
static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();
/// 进程级共享的 DoH 客户端（复用到 DoH 服务的连接）。
/// Process-wide DoH client (reuses connections to the DoH services).
static DOH_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn doh_client() -> &'static reqwest::Client {
    DOH_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(DOH_TIMEOUT)
            .no_proxy()
            .build()
            .unwrap_or_default()
    })
}

/// 基于 DoH 的 DNS 解析器，同时可用于 reqwest 与 wreq 客户端。
/// DoH-backed DNS resolver usable by both reqwest and wreq clients.
#[derive(Clone, Copy, Default)]
pub struct DohResolver;

impl DohResolver {
    /// 创建解析器。/ Create the resolver.
    pub fn new() -> Self {
        Self
    }

    /// 解析主机名，优先使用 DoH，失败时回退到系统解析。
    /// Resolve a hostname, preferring DoH and falling back to the system resolver.
    async fn resolve_host(&self, host: &str) -> Result<Vec<SocketAddr>, BoxError> {
        // 先查缓存 / Check the cache first
        if let Some(addrs) = self.cached(host) {
            return Ok(addrs);
        }

        // 并行查询两个 DoH 服务，取最先成功的结果 / Race both DoH providers, take the first success
        let mut tasks = tokio::task::JoinSet::new();
        for base in [
            "https://1.1.1.1/dns-query?name=",
            "https://dns.google/resolve?name=",
        ] {
            let client = doh_client().clone();
            let url = format!("{base}{host}&type=");
            tasks.spawn(async move { query_both_records(&client, &url).await });
        }

        let mut addrs: Vec<IpAddr> = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            if let Ok(Ok(found)) = joined
                && !found.is_empty()
            {
                addrs = found;
                tasks.abort_all();
                break;
            }
        }

        // DoH 不可用时回退到系统解析 / Fall back to the system resolver when DoH is unavailable
        if addrs.is_empty() {
            let resolved = tokio::net::lookup_host((host, 0)).await?;
            addrs = resolved.map(|addr| addr.ip()).collect();
        }

        let addrs: Vec<SocketAddr> = addrs
            .into_iter()
            .map(|ip| SocketAddr::new(ip, 0))
            .collect();

        if addrs.is_empty() {
            return Err(format!("DNS resolution for {host} returned no address").into());
        }

        cache().lock().insert(
            host.to_string(),
            CacheEntry {
                addrs: addrs.clone(),
                expires: Instant::now() + CACHE_TTL,
            },
        );
        Ok(addrs)
    }

    /// 读取未过期的缓存结果。/ Read a non-expired cached answer.
    fn cached(&self, host: &str) -> Option<Vec<SocketAddr>> {
        let mut entries = cache().lock();
        match entries.get(host) {
            Some(entry) if entry.expires > Instant::now() => Some(entry.addrs.clone()),
            Some(_) => {
                entries.remove(host);
                None
            }
            None => None,
        }
    }
}

/// 错误类型别名，兼容 reqwest / wreq 的 `BoxError`。
/// Error alias compatible with the `BoxError` used by reqwest / wreq.
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// 查询 A 记录，查不到时再补查 AAAA 记录。
/// 优先 IPv4 可以省掉一次往返，同时保留 IPv6-only 网络下的可用性。
///
/// Query the A record, and only fall back to AAAA when no IPv4 answer exists.
/// Preferring IPv4 saves a round trip while still working on IPv6-only networks.
async fn query_both_records(
    client: &reqwest::Client,
    url_prefix: &str,
) -> Result<Vec<IpAddr>, BoxError> {
    let v4_url = format!("{url_prefix}A");
    if let Some(v4) = query_record(client, &v4_url).await
        && !v4.is_empty()
    {
        return Ok(v4);
    }

    let v6_url = format!("{url_prefix}AAAA");
    Ok(query_record(client, &v6_url).await.unwrap_or_default())
}

/// 查询单个记录类型。/ Query a single record type.
async fn query_record(client: &reqwest::Client, url: &str) -> Option<Vec<IpAddr>> {
    let resp = client
        .get(url)
        .header("accept", "application/dns-json")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    let answers = json["Answer"].as_array()?;

    let addrs: Vec<IpAddr> = answers
        .iter()
        .filter_map(|answer| answer["data"].as_str())
        .filter_map(|data| data.trim().parse::<IpAddr>().ok())
        .collect();

    Some(addrs)
}

/// 为 reqwest 客户端提供自定义解析。/ Custom resolver for reqwest clients.
impl reqwest::dns::Resolve for DohResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        let this = self.clone();
        Box::pin(async move {
            let addrs = this.resolve_host(&host).await?;
            let addrs: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(addrs)
        })
    }
}

/// 为 wreq 客户端提供自定义解析。/ Custom resolver for wreq clients.
impl wreq::dns::Resolve for DohResolver {
    fn resolve(&self, name: wreq::dns::Name) -> wreq::dns::Resolving {
        let host = name.as_str().to_string();
        let this = self.clone();
        Box::pin(async move {
            let addrs = this.resolve_host(&host).await?;
            let addrs: wreq::dns::Addrs = Box::new(addrs.into_iter());
            Ok(addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_roundtrip() {
        let resolver = DohResolver::new();
        let addr = SocketAddr::new(IpAddr::from([1, 2, 3, 4]), 0);
        cache().lock().insert(
            "example.com".into(),
            CacheEntry {
                addrs: vec![addr],
                expires: Instant::now() + CACHE_TTL,
            },
        );
        assert_eq!(resolver.cached("example.com"), Some(vec![addr]));
        assert_eq!(resolver.cached("other.com"), None);
    }

    #[test]
    fn expired_entry_is_dropped() {
        let resolver = DohResolver::new();
        cache().lock().insert(
            "expired.com".into(),
            CacheEntry {
                addrs: vec![SocketAddr::new(IpAddr::from([1, 1, 1, 1]), 0)],
                expires: Instant::now() - Duration::from_secs(1),
            },
        );
        assert_eq!(resolver.cached("expired.com"), None);
        assert!(cache().lock().get("expired.com").is_none());
    }

    #[test]
    fn parses_doh_answers() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"Status":0,"Answer":[{"name":"a.com","type":1,"data":"1.2.3.4"},{"name":"a.com","type":28,"data":"::1"}]}"#,
        )
        .unwrap();
        let addrs: Vec<IpAddr> = json["Answer"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|answer| answer["data"].as_str())
            .filter_map(|data| data.trim().parse::<IpAddr>().ok())
            .collect();
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0].to_string(), "1.2.3.4");
    }
}
