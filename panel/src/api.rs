//! JSON API。权限模型：
//! - admin：管理所有网络、用户、planet 与全局设置
//! - user：只能看到并管理自己创建（或被分配）的网络，不能管理用户
//!
//! 无归属的网络（例如从旧版迁移来的）只有管理员可见

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::sync::Mutex;

use crate::auth::{self, create_token, now_secs, verify_password, verify_token, SESSION_TTL_SECS};
use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::ip;
use crate::planet::{self, PlanetConfig};
use crate::store::{NetworkMeta, Role, Store, User};
use crate::zt::ZtClient;

const COOKIE: &str = "ztp_session";

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub store: Arc<Mutex<Store>>,
    pub zt: ZtClient,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/me", get(me))
        .route("/api/me/password", post(change_password))
        .route("/api/info", get(info))
        .route("/api/settings", get(get_settings).patch(update_settings))
        .route("/api/users", get(list_users).post(create_user))
        .route("/api/users/{name}", axum::routing::patch(update_user).delete(delete_user))
        .route("/api/networks", get(list_networks).post(create_network))
        .route("/api/networks/{nwid}", get(get_network).patch(update_network).delete(delete_network))
        .route("/api/networks/{nwid}/rules", put(update_rules))
        .route("/api/networks/{nwid}/easy", post(easy_setup))
        .route("/api/networks/{nwid}/owner", put(set_owner))
        .route("/api/networks/{nwid}/members", post(add_member))
        .route("/api/networks/{nwid}/members/{mid}", axum::routing::patch(update_member).delete(delete_member))
        .route("/api/networks/{nwid}/members/{mid}/next-ip", post(assign_next_ip))
        .route("/api/planet", get(get_planet).put(update_planet))
        .route("/api/files/{name}", get(download_file))
}

// ---------------------------------------------------------------- 会话

pub struct Session {
    pub username: String,
    pub role: Role,
    pub must_change: bool,
}

impl Session {
    fn is_admin(&self) -> bool {
        self.role == Role::Admin
    }
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

async fn load_session(parts: &Parts, st: &AppState) -> AppResult<Session> {
    let token = cookie_token(&parts.headers).ok_or_else(AppError::unauthorized)?;
    let store = st.store.lock().await;
    let username = verify_token(&store.data.secret, &token, |n| store.data.users.get(n)).ok_or_else(AppError::unauthorized)?;
    let user = &store.data.users[&username];
    Ok(Session { role: user.role.clone(), must_change: user.must_change, username })
}

/// 允许“需要修改密码”状态的会话，仅用于查看自己、改密码
pub struct AnySession(pub Session);

impl FromRequestParts<AppState> for AnySession {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        Ok(AnySession(load_session(parts, st).await?))
    }
}

impl FromRequestParts<AppState> for Session {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let s = load_session(parts, st).await?;
        if s.must_change {
            return Err(AppError::new(StatusCode::FORBIDDEN, "请先修改初始密码"));
        }
        Ok(s)
    }
}

pub struct Admin(pub Session);

impl FromRequestParts<AppState> for Admin {
    type Rejection = AppError;
    async fn from_request_parts(parts: &mut Parts, st: &AppState) -> Result<Self, Self::Rejection> {
        let s = Session::from_request_parts(parts, st).await?;
        if !s.is_admin() {
            return Err(AppError::forbidden());
        }
        Ok(Admin(s))
    }
}

fn session_cookie(headers: &HeaderMap, value: &str, max_age: u64) -> HeaderValue {
    let secure = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https");
    let c = format!(
        "{COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    );
    HeaderValue::from_str(&c).expect("cookie header")
}

fn me_json(username: &str, u: &User) -> Value {
    json!({ "username": username, "role": u.role, "mustChange": u.must_change })
}

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

async fn login(State(st): State<AppState>, headers: HeaderMap, Json(req): Json<LoginReq>) -> AppResult<Response> {
    let ok = {
        let store = st.store.lock().await;
        store
            .data
            .users
            .get(&req.username)
            .filter(|u| verify_password(u, &req.password))
            .map(|u| (create_token(&store.data.secret, &req.username, u), me_json(&req.username, u)))
    };
    let Some((token, me)) = ok else {
        // 简单的失败延迟，减缓暴力破解
        tokio::time::sleep(Duration::from_millis(800)).await;
        return Err(AppError::new(StatusCode::UNAUTHORIZED, "用户名或密码错误"));
    };
    Ok(([(header::SET_COOKIE, session_cookie(&headers, &token, SESSION_TTL_SECS))], Json(me)).into_response())
}

async fn logout(headers: HeaderMap) -> Response {
    ([(header::SET_COOKIE, session_cookie(&headers, "", 0))], Json(json!({ "ok": true }))).into_response()
}

async fn me(State(st): State<AppState>, AnySession(s): AnySession) -> AppResult<Json<Value>> {
    let store = st.store.lock().await;
    Ok(Json(me_json(&s.username, &store.data.users[&s.username])))
}

#[derive(Deserialize)]
struct PasswordReq {
    old: String,
    new: String,
}

async fn change_password(
    State(st): State<AppState>,
    headers: HeaderMap,
    AnySession(s): AnySession,
    Json(req): Json<PasswordReq>,
) -> AppResult<Response> {
    let mut store = st.store.lock().await;
    let secret = store.data.secret.clone();
    let user = store.data.users.get_mut(&s.username).ok_or_else(AppError::unauthorized)?;
    if !verify_password(user, &req.old) {
        return Err(AppError::bad_request("原密码错误"));
    }
    if req.old == req.new {
        return Err(AppError::bad_request("新密码不能与原密码相同"));
    }
    auth::set_password(user, &req.new, false)?;
    let token = create_token(&secret, &s.username, user);
    let me = me_json(&s.username, user);
    store.save()?;
    Ok(([(header::SET_COOKIE, session_cookie(&headers, &token, SESSION_TTL_SECS))], Json(me)).into_response())
}

// ---------------------------------------------------------------- 概览

fn request_host(headers: &HeaderMap) -> String {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("127.0.0.1");
    if host.starts_with('[') {
        host.split(']').next().map(|h| format!("{h}]")).unwrap_or_default()
    } else {
        host.split(':').next().unwrap_or(host).to_string()
    }
}

fn moon_files(cfg: &Config) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(&cfg.dist_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".moon"))
        .collect();
    out.sort();
    out
}

async fn info(State(st): State<AppState>, headers: HeaderMap, s: Session) -> AppResult<Json<Value>> {
    let cfg = &st.cfg;
    let status = st.zt.status().await.ok();
    let planet = planet::load(cfg)?;
    let settings = st.store.lock().await.data.settings.clone();

    let file_base = if !settings.public_file_url.is_empty() {
        settings.public_file_url.clone()
    } else {
        let ip4 = cfg.read_config_file("ip_addr4");
        let ip6 = cfg.read_config_file("ip_addr6");
        let host = if !ip4.is_empty() {
            ip4
        } else if !ip6.is_empty() {
            format!("[{ip6}]")
        } else {
            request_host(&headers)
        };
        format!("http://{host}:{}", cfg.file_port)
    };
    let key = cfg.read_config_file("file_server.key");
    let link = |name: &str| format!("{file_base}/{name}?key={key}");

    Ok(Json(json!({
        "node": status.as_ref().map(|v| json!({
            "address": v["address"], "version": v["version"], "online": v["online"],
        })),
        "ztPort": cfg.zt_port,
        "roots": planet.roots,
        "files": {
            "planet": link("planet"),
            "moons": moon_files(cfg).iter().map(|m| json!({ "name": m, "url": link(m) })).collect::<Vec<_>>(),
        },
        "settings": settings,
        "isAdmin": s.is_admin(),
    })))
}

// ---------------------------------------------------------------- 设置

async fn get_settings(State(st): State<AppState>, _a: Admin) -> AppResult<Json<Value>> {
    Ok(Json(serde_json::to_value(&st.store.lock().await.data.settings)?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsReq {
    allow_user_create_network: Option<bool>,
    public_url: Option<String>,
    public_file_url: Option<String>,
}

fn clean_url(u: &str) -> AppResult<String> {
    let u = u.trim().trim_end_matches('/').to_string();
    if !u.is_empty() && !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err(AppError::bad_request("地址必须以 http:// 或 https:// 开头"));
    }
    Ok(u)
}

async fn update_settings(State(st): State<AppState>, _a: Admin, Json(req): Json<SettingsReq>) -> AppResult<Json<Value>> {
    let mut store = st.store.lock().await;
    if let Some(v) = req.allow_user_create_network {
        store.data.settings.allow_user_create_network = v;
    }
    if let Some(v) = req.public_url {
        store.data.settings.public_url = clean_url(&v)?;
    }
    if let Some(v) = req.public_file_url {
        store.data.settings.public_file_url = clean_url(&v)?;
    }
    store.save()?;
    Ok(Json(serde_json::to_value(&store.data.settings)?))
}

// ---------------------------------------------------------------- 用户（仅管理员）

async fn list_users(State(st): State<AppState>, _a: Admin) -> AppResult<Json<Value>> {
    let store = st.store.lock().await;
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for meta in store.data.networks.values() {
        if let Some(o) = &meta.owner {
            *counts.entry(o.as_str()).or_default() += 1;
        }
    }
    let users: Vec<Value> = store
        .data
        .users
        .iter()
        .map(|(name, u)| {
            json!({
                "username": name, "role": u.role, "mustChange": u.must_change,
                "created": u.created, "networks": counts.get(name.as_str()).copied().unwrap_or(0),
            })
        })
        .collect();
    Ok(Json(json!(users)))
}

#[derive(Deserialize)]
struct CreateUserReq {
    username: String,
    password: String,
    role: Role,
}

async fn create_user(State(st): State<AppState>, _a: Admin, Json(req): Json<CreateUserReq>) -> AppResult<Json<Value>> {
    auth::check_username(&req.username)?;
    let mut store = st.store.lock().await;
    if store.data.users.contains_key(&req.username) {
        return Err(AppError::conflict("用户已存在"));
    }
    // 管理员创建的账号首次登录需要自行修改密码
    let user = auth::new_user(&req.password, req.role, true)?;
    let out = me_json(&req.username, &user);
    store.data.users.insert(req.username, user);
    store.save()?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct UpdateUserReq {
    role: Option<Role>,
    password: Option<String>,
}

async fn update_user(
    State(st): State<AppState>,
    Admin(a): Admin,
    Path(name): Path<String>,
    Json(req): Json<UpdateUserReq>,
) -> AppResult<Json<Value>> {
    let mut store = st.store.lock().await;
    let admins = store.admin_count();
    let user = store.data.users.get_mut(&name).ok_or_else(AppError::not_found)?;
    if let Some(role) = req.role {
        if user.role == Role::Admin && role != Role::Admin {
            if name == a.username {
                return Err(AppError::bad_request("不能取消自己的管理员权限"));
            }
            if admins <= 1 {
                return Err(AppError::bad_request("至少需要保留一个管理员"));
            }
        }
        if user.role != role {
            user.role = role;
            user.token_version += 1;
        }
    }
    if let Some(pw) = req.password {
        // 管理员重置他人密码后，对方下次登录需修改
        auth::set_password(user, &pw, name != a.username)?;
    }
    let out = me_json(&name, user);
    store.save()?;
    Ok(Json(out))
}

async fn delete_user(State(st): State<AppState>, Admin(a): Admin, Path(name): Path<String>) -> AppResult<Json<Value>> {
    if name == a.username {
        return Err(AppError::bad_request("不能删除自己"));
    }
    let mut store = st.store.lock().await;
    if store.data.users.remove(&name).is_none() {
        return Err(AppError::not_found());
    }
    // 被删用户的网络转给执行删除的管理员，避免网络变成无人管理
    let mut moved = 0;
    for meta in store.data.networks.values_mut() {
        if meta.owner.as_deref() == Some(name.as_str()) {
            meta.owner = Some(a.username.clone());
            moved += 1;
        }
    }
    store.save()?;
    Ok(Json(json!({ "ok": true, "networksMovedTo": a.username, "networksMoved": moved })))
}

// ---------------------------------------------------------------- 网络

fn check_hex(s: &str, len: usize, what: &str) -> AppResult<()> {
    if s.len() == len && s.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::bad_request(format!("{what} 格式错误")))
    }
}

/// 校验当前用户能否访问该网络。无权访问时返回 404，避免暴露网络是否存在
async fn require_network(st: &AppState, s: &Session, nwid: &str) -> AppResult<NetworkMeta> {
    check_hex(nwid, 16, "网络 ID")?;
    let store = st.store.lock().await;
    let meta = store.data.networks.get(nwid).cloned().unwrap_or_default();
    if s.is_admin() || meta.owner.as_deref() == Some(s.username.as_str()) {
        Ok(meta)
    } else {
        Err(AppError::not_found())
    }
}

fn meta_json(meta: &NetworkMeta) -> Value {
    json!({ "owner": meta.owner, "description": meta.description, "rulesSource": meta.rules_source })
}

async fn list_networks(State(st): State<AppState>, s: Session) -> AppResult<Json<Value>> {
    let ids = st.zt.list_networks().await?;
    let metas = st.store.lock().await.data.networks.clone();
    let mut out = Vec::new();
    for id in ids {
        let meta = metas.get(&id).cloned().unwrap_or_default();
        if !s.is_admin() && meta.owner.as_deref() != Some(s.username.as_str()) {
            continue;
        }
        let net = match st.zt.get_network(&id).await {
            Ok(n) => n,
            Err(e) if e.status == StatusCode::NOT_FOUND => continue,
            Err(e) => return Err(e),
        };
        let members = st.zt.list_members(&id).await.unwrap_or_default();
        let authorized = members.iter().filter(|m| m["authorized"].as_bool() == Some(true)).count();
        out.push(json!({
            "id": id,
            "name": net["name"],
            "private": net["private"],
            "routes": net["routes"],
            "memberCount": members.len(),
            "authorizedCount": authorized,
            "meta": meta_json(&meta),
        }));
    }
    Ok(Json(json!(out)))
}

#[derive(Deserialize)]
struct CreateNetworkReq {
    name: String,
    #[serde(default)]
    description: String,
    cidr: Option<String>,
    start: Option<String>,
    end: Option<String>,
}

async fn create_network(State(st): State<AppState>, s: Session, Json(req): Json<CreateNetworkReq>) -> AppResult<Json<Value>> {
    if !s.is_admin() && !st.store.lock().await.data.settings.allow_user_create_network {
        return Err(AppError::new(StatusCode::FORBIDDEN, "管理员已禁止普通用户创建网络"));
    }
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("网络名称不能为空"));
    }
    let mut body = json!({ "name": name, "private": true });
    if let Some(cidr) = req.cidr.as_deref().filter(|c| !c.trim().is_empty()) {
        let easy = ip::easy_setup(cidr, req.start.as_deref(), req.end.as_deref())?;
        merge(&mut body, easy);
    }
    let net = st.zt.create_network(&body).await?;
    let id = net["id"].as_str().or(net["nwid"].as_str()).ok_or_else(|| AppError::bad_gateway("controller 未返回网络 ID"))?.to_string();
    let mut store = st.store.lock().await;
    store.data.networks.insert(
        id.clone(),
        NetworkMeta { owner: Some(s.username.clone()), description: req.description.trim().to_string(), rules_source: None, created: now_secs() },
    );
    store.save()?;
    Ok(Json(json!({ "id": id, "network": net })))
}

fn merge(dst: &mut Value, src: Value) {
    if let (Some(d), Value::Object(s)) = (dst.as_object_mut(), src) {
        d.extend(s);
    }
}

/// 汇总 /peer 信息，判断成员在线状态、物理地址、延迟、客户端版本
fn presence_map(peers: &Value) -> HashMap<String, Value> {
    let mut map = HashMap::new();
    for p in peers.as_array().into_iter().flatten() {
        let Some(addr) = p["address"].as_str() else { continue };
        let paths: Vec<&Value> = p["paths"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
        let live: Vec<&&Value> = paths.iter().filter(|x| x["expired"].as_bool() != Some(true)).collect();
        let preferred = live.iter().find(|x| x["preferred"].as_bool() == Some(true)).or(live.first());
        map.insert(
            addr.to_string(),
            json!({
                "online": !live.is_empty(),
                "physicalAddress": preferred.map(|x| x["address"].clone()),
                "latency": p["latency"],
                "version": p["version"],
                "role": p["role"],
            }),
        );
    }
    map
}

async fn get_network(State(st): State<AppState>, s: Session, Path(nwid): Path<String>) -> AppResult<Json<Value>> {
    let meta = require_network(&st, &s, &nwid).await?;
    let network = st.zt.get_network(&nwid).await?;
    let mut members = st.zt.list_members(&nwid).await?;
    let presence = presence_map(&st.zt.peers().await.unwrap_or(Value::Null));
    for m in members.iter_mut() {
        let id = m["id"].as_str().or(m["address"].as_str()).unwrap_or("").to_string();
        m["presence"] = presence.get(&id).cloned().unwrap_or(json!({ "online": false }));
    }
    members.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let users: Vec<String> = if s.is_admin() { st.store.lock().await.data.users.keys().cloned().collect() } else { vec![] };
    Ok(Json(json!({ "network": network, "members": members, "meta": meta_json(&meta), "users": users })))
}

/// 允许通过面板修改的网络字段
const NETWORK_FIELDS: &[&str] = &[
    "name", "private", "enableBroadcast", "mtu", "multicastLimit",
    "v4AssignMode", "v6AssignMode", "routes", "ipAssignmentPools", "dns",
];

async fn update_network(
    State(st): State<AppState>,
    s: Session,
    Path(nwid): Path<String>,
    Json(req): Json<Map<String, Value>>,
) -> AppResult<Json<Value>> {
    require_network(&st, &s, &nwid).await?;
    let body: Map<String, Value> = req.iter().filter(|(k, _)| NETWORK_FIELDS.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
    if let Some(routes) = body.get("routes").and_then(Value::as_array) {
        for r in routes {
            let target = r["target"].as_str().unwrap_or("");
            let (ip, bits) = target.split_once('/').ok_or_else(|| AppError::bad_request(format!("路由目标格式错误: {target}")))?;
            if ip::canonical_ip(ip).is_none() || bits.parse::<u8>().is_err() {
                return Err(AppError::bad_request(format!("路由目标格式错误: {target}")));
            }
            if let Some(via) = r["via"].as_str().filter(|v| !v.is_empty()) {
                ip::canonical_ip(via).ok_or_else(|| AppError::bad_request(format!("网关地址错误: {via}")))?;
            }
        }
    }
    let network = if body.is_empty() { st.zt.get_network(&nwid).await? } else { st.zt.update_network(&nwid, &Value::Object(body)).await? };
    if let Some(desc) = req.get("description").and_then(Value::as_str) {
        let mut store = st.store.lock().await;
        let meta = store.data.networks.entry(nwid.clone()).or_default();
        meta.description = desc.trim().to_string();
        store.save()?;
    }
    Ok(Json(network))
}

async fn delete_network(State(st): State<AppState>, s: Session, Path(nwid): Path<String>) -> AppResult<Json<Value>> {
    require_network(&st, &s, &nwid).await?;
    st.zt.delete_network(&nwid).await?;
    let mut store = st.store.lock().await;
    store.data.networks.remove(&nwid);
    store.save()?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct RulesReq {
    source: String,
    rules: Vec<Value>,
    #[serde(default)]
    capabilities: Vec<Value>,
    #[serde(default)]
    tags: Vec<Value>,
}

/// 规则在浏览器中用官方 rule-compiler.js 编译，这里保存编译结果与源码
async fn update_rules(State(st): State<AppState>, s: Session, Path(nwid): Path<String>, Json(req): Json<RulesReq>) -> AppResult<Json<Value>> {
    require_network(&st, &s, &nwid).await?;
    if req.rules.is_empty() {
        return Err(AppError::bad_request("规则为空，至少需要一条 accept 或 drop"));
    }
    if req.source.len() > 64 * 1024 {
        return Err(AppError::bad_request("规则源码过长"));
    }
    let body = json!({ "rules": req.rules, "capabilities": req.capabilities, "tags": req.tags });
    let network = st.zt.update_network(&nwid, &body).await?;
    let mut store = st.store.lock().await;
    store.data.networks.entry(nwid.clone()).or_default().rules_source = Some(req.source);
    store.save()?;
    Ok(Json(network))
}

#[derive(Deserialize)]
struct EasyReq {
    cidr: String,
    start: Option<String>,
    end: Option<String>,
}

async fn easy_setup(State(st): State<AppState>, s: Session, Path(nwid): Path<String>, Json(req): Json<EasyReq>) -> AppResult<Json<Value>> {
    require_network(&st, &s, &nwid).await?;
    let body = ip::easy_setup(&req.cidr, req.start.as_deref(), req.end.as_deref())?;
    Ok(Json(st.zt.update_network(&nwid, &body).await?))
}

#[derive(Deserialize)]
struct OwnerReq {
    owner: String,
}

async fn set_owner(State(st): State<AppState>, Admin(_): Admin, Path(nwid): Path<String>, Json(req): Json<OwnerReq>) -> AppResult<Json<Value>> {
    check_hex(&nwid, 16, "网络 ID")?;
    st.zt.get_network(&nwid).await?;
    let mut store = st.store.lock().await;
    if !store.data.users.contains_key(&req.owner) {
        return Err(AppError::bad_request("用户不存在"));
    }
    let meta = store.data.networks.entry(nwid).or_default();
    meta.owner = Some(req.owner);
    let out = meta_json(meta);
    store.save()?;
    Ok(Json(out))
}

// ---------------------------------------------------------------- 成员

#[derive(Deserialize)]
struct AddMemberReq {
    id: String,
    #[serde(default)]
    authorized: bool,
    #[serde(default)]
    name: String,
}

/// 手动添加成员（可提前授权，设备加入时直接生效）
async fn add_member(State(st): State<AppState>, s: Session, Path(nwid): Path<String>, Json(req): Json<AddMemberReq>) -> AppResult<Json<Value>> {
    require_network(&st, &s, &nwid).await?;
    let mid = req.id.trim().to_lowercase();
    check_hex(&mid, 10, "设备 ID")?;
    let body = json!({ "authorized": req.authorized, "name": req.name.trim() });
    Ok(Json(st.zt.update_member(&nwid, &mid, &body).await?))
}

async fn update_member(
    State(st): State<AppState>,
    s: Session,
    Path((nwid, mid)): Path<(String, String)>,
    Json(req): Json<Map<String, Value>>,
) -> AppResult<Json<Value>> {
    require_network(&st, &s, &nwid).await?;
    check_hex(&mid, 10, "设备 ID")?;
    let mut body = Map::new();
    for key in ["authorized", "activeBridge", "noAutoAssignIps"] {
        if let Some(v) = req.get(key) {
            let b = v.as_bool().ok_or_else(|| AppError::bad_request(format!("{key} 必须是布尔值")))?;
            body.insert(key.into(), json!(b));
        }
    }
    if let Some(v) = req.get("name") {
        let name: String = v.as_str().unwrap_or("").trim().chars().take(64).collect();
        body.insert("name".into(), json!(name));
    }
    if let Some(v) = req.get("ipAssignments") {
        let list = v.as_array().ok_or_else(|| AppError::bad_request("ipAssignments 必须是数组"))?;
        body.insert("ipAssignments".into(), json!(ip::normalize_ip_list(list)?));
    }
    if body.is_empty() {
        return Err(AppError::bad_request("没有可修改的字段"));
    }
    Ok(Json(st.zt.update_member(&nwid, &mid, &Value::Object(body)).await?))
}

async fn delete_member(State(st): State<AppState>, s: Session, Path((nwid, mid)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_network(&st, &s, &nwid).await?;
    check_hex(&mid, 10, "设备 ID")?;
    st.zt.delete_member(&nwid, &mid).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn assign_next_ip(State(st): State<AppState>, s: Session, Path((nwid, mid)): Path<(String, String)>) -> AppResult<Json<Value>> {
    require_network(&st, &s, &nwid).await?;
    check_hex(&mid, 10, "设备 ID")?;
    let network = st.zt.get_network(&nwid).await?;
    let members = st.zt.list_members(&nwid).await?;
    let ip = ip::next_free_ip(&network, &members).ok_or_else(|| AppError::bad_request("地址池中没有可用 IP，请先设置地址池"))?;
    let member = members.iter().find(|m| m["id"].as_str() == Some(mid.as_str())).ok_or_else(AppError::not_found)?;
    let mut list = member["ipAssignments"].as_array().cloned().unwrap_or_default();
    list.push(json!(ip));
    let body = json!({ "ipAssignments": ip::normalize_ip_list(&list)? });
    Ok(Json(st.zt.update_member(&nwid, &mid, &body).await?))
}

// ---------------------------------------------------------------- planet（仅管理员）

async fn get_planet(State(st): State<AppState>, _a: Admin) -> AppResult<Json<Value>> {
    let planet = planet::load(&st.cfg)?;
    let local = planet::local_address(&st.cfg).ok();
    Ok(Json(json!({ "roots": planet.roots, "local": local, "maxRoots": planet::MAX_ROOTS })))
}

async fn update_planet(State(st): State<AppState>, _a: Admin, Json(req): Json<PlanetConfig>) -> AppResult<Json<Value>> {
    let cfg = planet::validate(&req)?;
    let local = planet::local_address(&st.cfg)?;
    if !cfg.roots.iter().any(|r| r.address() == local) {
        return Err(AppError::bad_request(format!("不能移除本机根节点 {local}")));
    }
    planet::save_and_build(&st.cfg, &cfg).await?;
    // 重启后本机根节点加载新 planet，并向已连接客户端下发更新
    planet::restart_zerotier(&st.cfg).await?;
    Ok(Json(json!({ "ok": true, "roots": cfg.roots })))
}

// ---------------------------------------------------------------- 文件

pub fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.len() <= 128
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

pub fn file_response(cfg: &Config, name: &str) -> AppResult<Response> {
    if !safe_file_name(name) {
        return Err(AppError::not_found());
    }
    let path = cfg.dist_dir.join(name);
    if !path.is_file() {
        return Err(AppError::not_found());
    }
    let bytes = std::fs::read(path)?;
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream")),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")).map_err(|_| AppError::not_found())?,
            ),
        ],
        bytes,
    )
        .into_response())
}

async fn download_file(State(st): State<AppState>, _s: Session, Path(name): Path<String>) -> AppResult<Response> {
    file_response(&st.cfg, &name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        assert!(safe_file_name("planet"));
        assert!(safe_file_name("000000abcdef.moon"));
        assert!(!safe_file_name("../etc/passwd"));
        assert!(!safe_file_name(".hidden"));
        assert!(!safe_file_name("a/b"));
    }

    #[test]
    fn presence() {
        let peers = json!([
            { "address": "aaaaaaaaaa", "latency": 12, "version": "1.16.2", "role": "LEAF",
              "paths": [{ "address": "1.1.1.1/1234", "expired": false, "preferred": true }] },
            { "address": "bbbbbbbbbb", "latency": -1, "paths": [{ "address": "2.2.2.2/1", "expired": true }] }
        ]);
        let m = presence_map(&peers);
        assert_eq!(m["aaaaaaaaaa"]["online"], true);
        assert_eq!(m["aaaaaaaaaa"]["physicalAddress"], "1.1.1.1/1234");
        assert_eq!(m["bbbbbbbbbb"]["online"], false);
    }

    #[test]
    fn cookie_parse() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; ztp_session=tok.en; b=2"));
        assert_eq!(cookie_token(&h).as_deref(), Some("tok.en"));
    }
}
