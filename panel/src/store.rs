//! 面板自身的数据：用户、网络归属与备注、规则源码、设置。
//! 数据量很小，整体放在内存里，每次修改后原子写回 JSON 文件。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::auth::random_hex;
use crate::error::AppResult;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub password_hash: String,
    pub role: Role,
    #[serde(default)]
    pub must_change: bool,
    /// 改密码、删除用户时递增，使旧会话失效
    #[serde(default)]
    pub token_version: u64,
    #[serde(default)]
    pub created: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkMeta {
    /// 网络所有者；为空表示仅管理员可见（例如迁移前创建的网络）
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub description: String,
    /// 规则源码。controller 只保存编译后的 JSON，源码由面板保存
    #[serde(default)]
    pub rules_source: Option<String>,
    #[serde(default)]
    pub created: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_true")]
    pub allow_user_create_network: bool,
    /// 对外访问地址，例如 https://example.com/zt，用于生成二维码和下载链接
    #[serde(default)]
    pub public_url: String,
    /// 文件服务对外地址，例如 https://example.com/zt-files，为空时用 http://<IP>:<端口>
    #[serde(default)]
    pub public_file_url: String,
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self { allow_user_create_network: true, public_url: String::new(), public_file_url: String::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Data {
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub users: BTreeMap<String, User>,
    #[serde(default)]
    pub networks: BTreeMap<String, NetworkMeta>,
    #[serde(default)]
    pub settings: Settings,
}

pub struct Store {
    path: PathBuf,
    pub data: Data,
}

impl Store {
    pub fn load(path: &Path) -> AppResult<Self> {
        let mut data: Data = if path.exists() {
            serde_json::from_str(&std::fs::read_to_string(path)?)?
        } else {
            Data::default()
        };
        if data.secret.is_empty() {
            data.secret = random_hex(32);
        }
        let store = Self { path: path.to_path_buf(), data };
        store.save()?;
        Ok(store)
    }

    pub fn save(&self) -> AppResult<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.data)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn admin_count(&self) -> usize {
        self.data.users.values().filter(|u| u.role == Role::Admin).count()
    }
}
