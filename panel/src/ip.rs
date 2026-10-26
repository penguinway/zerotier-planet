//! IP 相关工具：规范化、去重、快速设置、自动分配

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};

use serde_json::{json, Value};

use crate::error::{AppError, AppResult};

pub fn canonical_ip(s: &str) -> Option<String> {
    s.trim().parse::<IpAddr>().ok().map(|ip| ip.to_string())
}

/// 修复 ztncui 中 IP assignment 重复添加的问题：
/// 写入成员 ipAssignments 前统一规范化、校验并去重（保持原顺序）
pub fn normalize_ip_list(list: &[Value]) -> AppResult<Vec<String>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for v in list {
        let raw = v.as_str().ok_or_else(|| AppError::bad_request("IP 地址必须是字符串"))?;
        let ip = canonical_ip(raw).ok_or_else(|| AppError::bad_request(format!("无效的 IP 地址: {raw}")))?;
        if seen.insert(ip.clone()) {
            out.push(ip);
        }
    }
    Ok(out)
}

fn parse_v4(s: &str) -> AppResult<u32> {
    s.trim()
        .parse::<Ipv4Addr>()
        .map(u32::from)
        .map_err(|_| AppError::bad_request(format!("无效的 IPv4 地址: {s}")))
}

/// 由 CIDR 生成路由与 IPv4 地址池（“快速设置”）
pub fn easy_setup(cidr: &str, start: Option<&str>, end: Option<&str>) -> AppResult<Value> {
    let err = || AppError::bad_request("请输入 /8 ~ /30 之间的 IPv4 网段，例如 10.11.12.0/24");
    let (ip, bits) = cidr.trim().split_once('/').ok_or_else(err)?;
    let bits: u32 = bits.parse().map_err(|_| err())?;
    if !(8..=30).contains(&bits) {
        return Err(err());
    }
    let mask = u32::MAX << (32 - bits);
    let net = parse_v4(ip)? & mask;
    let first = net + 1;
    let last = (net | !mask) - 1;
    let s = match start.filter(|s| !s.trim().is_empty()) {
        Some(v) => parse_v4(v)?,
        None => first,
    };
    let e = match end.filter(|s| !s.trim().is_empty()) {
        Some(v) => parse_v4(v)?,
        None => last,
    };
    if s < first || e > last || s > e {
        return Err(AppError::bad_request(format!(
            "地址池必须位于 {} ~ {} 之间",
            Ipv4Addr::from(first),
            Ipv4Addr::from(last)
        )));
    }
    Ok(json!({
        "routes": [{ "target": format!("{}/{}", Ipv4Addr::from(net), bits), "via": null }],
        "ipAssignmentPools": [{ "ipRangeStart": Ipv4Addr::from(s).to_string(), "ipRangeEnd": Ipv4Addr::from(e).to_string() }],
        "v4AssignMode": { "zt": true },
    }))
}

/// 在网络的 IPv4 地址池里找一个没被任何成员占用的地址
pub fn next_free_ip(network: &Value, members: &[Value]) -> Option<String> {
    let used: HashSet<String> = members
        .iter()
        .flat_map(|m| m["ipAssignments"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| v.as_str().and_then(canonical_ip))
        .collect();
    for pool in network["ipAssignmentPools"].as_array().into_iter().flatten() {
        let (Some(s), Some(e)) = (pool["ipRangeStart"].as_str(), pool["ipRangeEnd"].as_str()) else { continue };
        let (Ok(s), Ok(e)) = (s.parse::<Ipv4Addr>(), e.parse::<Ipv4Addr>()) else { continue };
        for n in u32::from(s)..=u32::from(e) {
            let ip = Ipv4Addr::from(n).to_string();
            if !used.contains(&ip) {
                return Some(ip);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupes_and_canonicalizes() {
        let list = vec![json!("10.0.0.1"), json!(" 10.0.0.1 "), json!("fd00:0:0::1"), json!("fd00::1"), json!("10.0.0.2")];
        assert_eq!(normalize_ip_list(&list).unwrap(), vec!["10.0.0.1", "fd00::1", "10.0.0.2"]);
        assert!(normalize_ip_list(&[json!("10.0.0.300")]).is_err());
    }

    #[test]
    fn easy_setup_defaults_and_bounds() {
        let v = easy_setup("10.11.12.5/24", None, None).unwrap();
        assert_eq!(v["routes"][0]["target"], "10.11.12.0/24");
        assert_eq!(v["ipAssignmentPools"][0]["ipRangeStart"], "10.11.12.1");
        assert_eq!(v["ipAssignmentPools"][0]["ipRangeEnd"], "10.11.12.254");
        let v = easy_setup("10.11.12.0/24", Some("10.11.12.101"), Some("10.11.12.199")).unwrap();
        assert_eq!(v["ipAssignmentPools"][0]["ipRangeStart"], "10.11.12.101");
        assert!(easy_setup("10.11.12.0/24", Some("10.11.13.1"), None).is_err());
        assert!(easy_setup("10.0.0.0/31", None, None).is_err());
    }

    #[test]
    fn finds_next_free() {
        let net = json!({ "ipAssignmentPools": [{ "ipRangeStart": "10.0.0.1", "ipRangeEnd": "10.0.0.3" }] });
        let members = vec![json!({ "ipAssignments": ["10.0.0.1"] }), json!({ "ipAssignments": ["10.0.0.2"] })];
        assert_eq!(next_free_ip(&net, &members).as_deref(), Some("10.0.0.3"));
        let members = vec![json!({ "ipAssignments": ["10.0.0.1", "10.0.0.2", "10.0.0.3"] })];
        assert_eq!(next_free_ip(&net, &members), None);
    }
}
