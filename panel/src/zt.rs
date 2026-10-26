//! ZeroTier 本地 API（含内嵌 controller）客户端

use std::path::PathBuf;
use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde_json::Value;

use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct ZtClient {
    base: String,
    token_file: PathBuf,
    http: reqwest::Client,
}

impl ZtClient {
    pub fn new(port: u16, token_file: PathBuf) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .no_proxy()
            .build()
            .expect("http client");
        Self { base: format!("http://127.0.0.1:{port}"), token_file, http }
    }

    fn token(&self) -> AppResult<String> {
        std::fs::read_to_string(&self.token_file)
            .map(|s| s.trim().to_string())
            .map_err(|e| AppError::bad_gateway(format!("读取 authtoken.secret 失败: {e}")))
    }

    async fn request(&self, method: Method, path: &str, body: Option<&Value>) -> AppResult<Value> {
        let mut req = self.http.request(method, format!("{}{}", self.base, path)).header("X-ZT1-Auth", self.token()?);
        if let Some(b) = body {
            req = req.json(b);
        }
        let res = req.send().await.map_err(|e| AppError::bad_gateway(format!("无法连接 zerotier-one: {e}")))?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if status == StatusCode::NOT_FOUND {
            return Err(AppError::not_found());
        }
        if !status.is_success() {
            let snippet: String = text.chars().take(200).collect();
            return Err(AppError::bad_gateway(format!("zerotier-one 返回 {status}: {snippet}")));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| AppError::bad_gateway(format!("zerotier-one 返回了无效 JSON: {e}")))
    }

    pub async fn status(&self) -> AppResult<Value> {
        self.request(Method::GET, "/status", None).await
    }

    pub async fn address(&self) -> AppResult<String> {
        let status = self.status().await?;
        status["address"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| AppError::bad_gateway("zerotier-one 未返回节点地址"))
    }

    pub async fn peers(&self) -> AppResult<Value> {
        self.request(Method::GET, "/peer", None).await
    }

    pub async fn list_networks(&self) -> AppResult<Vec<String>> {
        let v = self.request(Method::GET, "/controller/network", None).await?;
        Ok(v.as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default())
    }

    pub async fn get_network(&self, nwid: &str) -> AppResult<Value> {
        self.request(Method::GET, &format!("/controller/network/{nwid}"), None).await
    }

    pub async fn create_network(&self, body: &Value) -> AppResult<Value> {
        let addr = self.address().await?;
        self.request(Method::POST, &format!("/controller/network/{addr}______"), Some(body)).await
    }

    pub async fn update_network(&self, nwid: &str, body: &Value) -> AppResult<Value> {
        self.request(Method::POST, &format!("/controller/network/{nwid}"), Some(body)).await
    }

    pub async fn delete_network(&self, nwid: &str) -> AppResult<Value> {
        self.request(Method::DELETE, &format!("/controller/network/{nwid}"), None).await
    }

    /// 优先使用一次返回全部成员的 /unstable 接口，不支持时回退到逐个拉取
    pub async fn list_members(&self, nwid: &str) -> AppResult<Vec<Value>> {
        match self.request(Method::GET, &format!("/unstable/controller/network/{nwid}/member"), None).await {
            Ok(v) => {
                if let Some(arr) = v["data"].as_array() {
                    return Ok(arr.clone());
                }
            }
            Err(e) if e.status == axum::http::StatusCode::NOT_FOUND => {}
            Err(e) => return Err(e),
        }
        let ids = self.request(Method::GET, &format!("/controller/network/{nwid}/member"), None).await?;
        let mut out = Vec::new();
        if let Some(map) = ids.as_object() {
            for mid in map.keys() {
                out.push(self.get_member(nwid, mid).await?);
            }
        }
        Ok(out)
    }

    pub async fn get_member(&self, nwid: &str, mid: &str) -> AppResult<Value> {
        self.request(Method::GET, &format!("/controller/network/{nwid}/member/{mid}"), None).await
    }

    pub async fn update_member(&self, nwid: &str, mid: &str, body: &Value) -> AppResult<Value> {
        self.request(Method::POST, &format!("/controller/network/{nwid}/member/{mid}"), Some(body)).await
    }

    pub async fn delete_member(&self, nwid: &str, mid: &str) -> AppResult<Value> {
        self.request(Method::DELETE, &format!("/controller/network/{nwid}/member/{mid}"), None).await
    }
}
