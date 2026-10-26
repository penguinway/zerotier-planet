use std::path::PathBuf;

/// 运行配置，全部来自环境变量，默认值与容器内路径一致
#[derive(Debug, Clone)]
pub struct Config {
    pub zt_home: PathBuf,
    pub config_dir: PathBuf,
    pub dist_dir: PathBuf,
    pub zt_port: u16,
    pub api_port: u16,
    pub file_port: u16,
    /// 反向代理子路径，例如 /console/zerotier；无前导/尾随斜杠问题，统一规范化
    pub base_path: String,
    pub mkworld: String,
    pub admin_password: String,
}

fn env(name: &str, default: &str) -> String {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| default.to_string())
}

fn env_port(name: &str, default: u16) -> u16 {
    env(name, "").parse().unwrap_or(default)
}

pub fn normalize_base_path(p: &str) -> String {
    let trimmed = p.trim().trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("/{trimmed}")
    }
}

impl Config {
    pub fn from_env() -> Self {
        let config_dir = PathBuf::from(env("CONFIG_DIR", "/app/config"));
        // 端口优先读取持久化配置（与 deploy.sh 一致），其次环境变量
        let saved_port = |file: &str| std::fs::read_to_string(config_dir.join(file)).ok().and_then(|s| s.trim().parse().ok());
        Self {
            zt_home: PathBuf::from(env("ZT_HOME", "/var/lib/zerotier-one")),
            dist_dir: PathBuf::from(env("DIST_DIR", "/app/dist")),
            zt_port: saved_port("zerotier-one.port").unwrap_or_else(|| env_port("ZT_PORT", 9994)),
            api_port: env_port("API_PORT", 3443),
            file_port: env_port("FILE_SERVER_PORT", 3000),
            base_path: normalize_base_path(&env("BASE_PATH", "")),
            mkworld: env("MKWORLD", "mkworld"),
            admin_password: env("ADMIN_PASSWORD", ""),
            config_dir,
        }
    }

    pub fn read_config_file(&self, name: &str) -> String {
        std::fs::read_to_string(self.config_dir.join(name)).map(|s| s.trim().to_string()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_base_path;

    #[test]
    fn base_path() {
        assert_eq!(normalize_base_path(""), "");
        assert_eq!(normalize_base_path("/"), "");
        assert_eq!(normalize_base_path("console/zt/"), "/console/zt");
        assert_eq!(normalize_base_path("/console/zt"), "/console/zt");
    }
}
