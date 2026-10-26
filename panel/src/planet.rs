//! 多根节点 planet 管理
//!
//! config/planet.json 保存根节点列表，修改后调用 mkworld 重新签名生成 planet。
//! 签名密钥（previous/current.c25519）保存在 zerotier 数据目录中且始终不变，
//! 因此新 planet 的时间戳更新后，已在使用旧 planet 的客户端会从根节点自动获取新版本。

use std::net::IpAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::config::Config;
use crate::error::{AppError, AppResult};

pub const MAX_ROOTS: usize = 4;
pub const MAX_ENDPOINTS: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Root {
    #[serde(default)]
    pub name: String,
    pub identity: String,
    #[serde(rename = "stableEndpoints")]
    pub stable_endpoints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlanetConfig {
    pub roots: Vec<Root>,
}

impl Root {
    pub fn address(&self) -> &str {
        self.identity.split(':').next().unwrap_or("")
    }
}

/// 只保留身份的公钥部分（地址:0:公钥），拒绝任何格式不符的输入
pub fn public_identity(identity: &str) -> AppResult<String> {
    let parts: Vec<&str> = identity.trim().split(':').collect();
    let hex = |s: &str, n: usize| s.len() == n && s.chars().all(|c| c.is_ascii_hexdigit());
    if parts.len() < 3 || !hex(parts[0], 10) || parts[1] != "0" || !hex(parts[2], 128) {
        return Err(AppError::bad_request("身份格式错误，应为 identity.public 的内容（地址:0:公钥）"));
    }
    Ok(format!("{}:0:{}", parts[0].to_lowercase(), parts[2].to_lowercase()))
}

/// 端点格式：IP/端口，IPv6 不加方括号，例如 1.2.3.4/9994、2001:db8::1/9994
pub fn normalize_endpoint(ep: &str) -> AppResult<String> {
    let err = || AppError::bad_request(format!("端点格式错误: {ep}，应为 IP/端口"));
    let (ip, port) = ep.trim().rsplit_once('/').ok_or_else(err)?;
    let ip: IpAddr = ip.trim_matches(|c| c == '[' || c == ']').parse().map_err(|_| err())?;
    let port: u16 = port.parse().map_err(|_| err())?;
    if port == 0 {
        return Err(err());
    }
    Ok(format!("{ip}/{port}"))
}

pub fn validate(cfg: &PlanetConfig) -> AppResult<PlanetConfig> {
    if cfg.roots.is_empty() {
        return Err(AppError::bad_request("至少需要一个根节点"));
    }
    if cfg.roots.len() > MAX_ROOTS {
        return Err(AppError::bad_request(format!("根节点最多 {MAX_ROOTS} 个")));
    }
    let mut out = PlanetConfig::default();
    for r in &cfg.roots {
        let identity = public_identity(&r.identity)?;
        if out.roots.iter().any(|x| x.identity == identity) {
            return Err(AppError::bad_request(format!("根节点 {} 重复", &identity[..10])));
        }
        let mut eps = Vec::new();
        for ep in &r.stable_endpoints {
            let ep = normalize_endpoint(ep)?;
            if !eps.contains(&ep) {
                eps.push(ep);
            }
        }
        if eps.is_empty() {
            return Err(AppError::bad_request(format!("根节点 {} 至少需要一个端点", &identity[..10])));
        }
        if eps.len() > MAX_ENDPOINTS {
            return Err(AppError::bad_request(format!("每个根节点最多 {MAX_ENDPOINTS} 个端点")));
        }
        out.roots.push(Root { name: r.name.trim().chars().take(64).collect(), identity, stable_endpoints: eps });
    }
    Ok(out)
}

/// 本机节点地址，直接读 identity.public（zerotier-one 重启期间也可用）
pub fn local_address(cfg: &Config) -> AppResult<String> {
    let id = std::fs::read_to_string(cfg.zt_home.join("identity.public"))
        .map_err(|e| AppError::internal(format!("读取 identity.public 失败: {e}")))?;
    let addr = id.trim().split(':').next().unwrap_or("").to_string();
    if addr.len() != 10 {
        return Err(AppError::internal("identity.public 内容无效"));
    }
    Ok(addr)
}

pub fn load(cfg: &Config) -> AppResult<PlanetConfig> {
    let path = cfg.config_dir.join("planet.json");
    if !path.exists() {
        return Ok(PlanetConfig::default());
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

/// 保存配置并生成 planet：写入 dist/planet（供下载）与 zerotier 数据目录（根节点自身使用）
pub async fn save_and_build(cfg: &Config, planet: &PlanetConfig) -> AppResult<()> {
    let planet = validate(planet)?;
    let path = cfg.config_dir.join("planet.json");
    let tmp_cfg = cfg.config_dir.join("planet.json.new");
    std::fs::write(&tmp_cfg, serde_json::to_vec_pretty(&planet)?)?;

    let out = cfg.config_dir.join("world.bin.new");
    let res = Command::new(&cfg.mkworld)
        .arg("-c")
        .arg(&tmp_cfg)
        .arg("-o")
        .arg(&out)
        .arg("-k")
        .arg(&cfg.zt_home)
        .output()
        .await
        .map_err(|e| AppError::internal(format!("无法执行 mkworld: {e}")))?;
    if !res.status.success() {
        let _ = std::fs::remove_file(&tmp_cfg);
        return Err(AppError::internal(format!("mkworld 失败: {}", String::from_utf8_lossy(&res.stderr).trim())));
    }

    std::fs::create_dir_all(&cfg.dist_dir)?;
    copy_atomic(&out, &cfg.dist_dir.join("planet"))?;
    copy_atomic(&out, &cfg.zt_home.join("planet"))?;
    std::fs::remove_file(&out)?;
    std::fs::rename(&tmp_cfg, &path)?;
    Ok(())
}

fn copy_atomic(src: &Path, dst: &Path) -> AppResult<()> {
    let tmp = dst.with_extension("tmp");
    std::fs::copy(src, &tmp)?;
    std::fs::rename(&tmp, dst)?;
    Ok(())
}

/// 重启 zerotier-one 让本机根节点加载新 planet（进程由 entrypoint 守护，退出后自动拉起）
pub async fn restart_zerotier(cfg: &Config) -> AppResult<()> {
    let pid = std::fs::read_to_string(cfg.zt_home.join("zerotier-one.pid"))
        .map_err(|e| AppError::internal(format!("读取 zerotier-one.pid 失败: {e}")))?;
    let pid = pid.trim();
    if pid.is_empty() || !pid.chars().all(|c| c.is_ascii_digit()) {
        return Err(AppError::internal("zerotier-one.pid 内容无效"));
    }
    let st = Command::new("kill").arg(pid).status().await?;
    if !st.success() {
        return Err(AppError::internal("重启 zerotier-one 失败"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0377e187ea:0:c85594da5ef782a9a225d2fdf4bb106937de3317ee3561d88631a1ad7caaac33d3614b6170e3c28b675e5d2308dbd4a6a28e923562fe20343dfde9d9090e5061";

    #[test]
    fn identity_strips_private_part() {
        let with_secret = format!("{ID}:{}", "ab".repeat(64));
        assert_eq!(public_identity(&with_secret).unwrap(), ID);
        assert!(public_identity("0377e187ea:0:abcd").is_err());
    }

    #[test]
    fn endpoints() {
        assert_eq!(normalize_endpoint("1.2.3.4/9994").unwrap(), "1.2.3.4/9994");
        assert_eq!(normalize_endpoint("[2001:db8:0::1]/9994").unwrap(), "2001:db8::1/9994");
        assert!(normalize_endpoint("1.2.3.4:9994").is_err());
        assert!(normalize_endpoint("example.com/9994").is_err());
    }

    #[test]
    fn validate_dedupes() {
        let cfg = PlanetConfig {
            roots: vec![Root { name: "a".into(), identity: ID.into(), stable_endpoints: vec!["1.2.3.4/9994".into(), "1.2.3.4/9994".into()] }],
        };
        assert_eq!(validate(&cfg).unwrap().roots[0].stable_endpoints.len(), 1);
        let dup = PlanetConfig { roots: vec![cfg.roots[0].clone(), cfg.roots[0].clone()] };
        assert!(validate(&dup).is_err());
    }
}
