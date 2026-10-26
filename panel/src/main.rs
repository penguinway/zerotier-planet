mod api;
mod auth;
mod config;
mod error;
mod ip;
mod planet;
mod store;
mod zt;

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Router, ServiceExt};
use rust_embed::RustEmbed;
use tokio::sync::Mutex;
use tower::Layer;

use crate::api::AppState;
use crate::config::Config;
use crate::error::{AppError, AppResult};
use crate::store::{Role, Store};
use crate::zt::ZtClient;

#[derive(RustEmbed)]
#[folder = "public/"]
struct Assets;

const DEFAULT_PASSWORD: &str = "password";

fn usage() -> ! {
    eprintln!(
        "用法:\n  ztpanel [serve]                 启动面板与文件服务\n  ztpanel reset-password [用户]   重置密码为 {DEFAULT_PASSWORD}（默认 admin），首次登录需修改\n  ztpanel planet-build            按 config/planet.json 生成 planet\n  ztpanel files                   仅启动文件服务"
    );
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    let cfg = Config::from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None | Some("serve") => serve(cfg).await,
        Some("reset-password") => reset_password(&cfg, args.get(1).map(String::as_str).unwrap_or("admin")),
        Some("planet-build") => planet_build(&cfg).await,
        Some("files") => serve_files(cfg).await,
        _ => usage(),
    };
    if let Err(e) = result {
        eprintln!("错误: {e}");
        std::process::exit(1);
    }
}

fn reset_password(cfg: &Config, username: &str) -> AppResult<()> {
    let mut store = Store::load(&cfg.config_dir.join("panel.json"))?;
    match store.data.users.get_mut(username) {
        Some(u) => auth::set_password(u, DEFAULT_PASSWORD, true)?,
        None => {
            auth::check_username(username)?;
            store.data.users.insert(username.to_string(), auth::new_user(DEFAULT_PASSWORD, Role::Admin, true)?);
        }
    }
    store.save()?;
    println!("用户 {username} 的密码已重置为 {DEFAULT_PASSWORD}，登录后需修改；面板运行中时请重启容器使其生效");
    Ok(())
}

async fn planet_build(cfg: &Config) -> AppResult<()> {
    let planet = planet::load(cfg)?;
    planet::save_and_build(cfg, &planet).await?;
    println!("planet 已生成，根节点 {} 个", planet.roots.len());
    Ok(())
}

/// 首次启动时创建管理员：设置了 ADMIN_PASSWORD 则直接使用，否则为默认密码且必须修改
fn ensure_admin(cfg: &Config, store: &mut Store) -> AppResult<()> {
    if !store.data.users.is_empty() {
        return Ok(());
    }
    let user = if cfg.admin_password.is_empty() {
        auth::new_user(DEFAULT_PASSWORD, Role::Admin, true)?
    } else {
        auth::new_user(&cfg.admin_password, Role::Admin, false)?
    };
    store.data.users.insert("admin".into(), user);
    store.save()
}

/// 文件服务密钥：优先 FILE_KEY，其次沿用已保存的密钥，都没有则生成并保存（重启后链接不变）
fn file_key(cfg: &Config) -> AppResult<String> {
    let key = std::env::var("FILE_KEY").ok().filter(|k| !k.trim().is_empty()).map(|k| k.trim().to_string());
    let key = key.unwrap_or_else(|| {
        let saved = cfg.read_config_file("file_server.key");
        if saved.is_empty() { auth::random_hex(16) } else { saved }
    });
    std::fs::create_dir_all(&cfg.config_dir)?;
    std::fs::write(cfg.config_dir.join("file_server.key"), &key)?;
    Ok(key)
}

async fn serve(cfg: Config) -> AppResult<()> {
    let mut store = Store::load(&cfg.config_dir.join("panel.json"))?;
    ensure_admin(&cfg, &mut store)?;
    let key = Arc::new(file_key(&cfg)?);

    let cfg = Arc::new(cfg);
    let state = AppState {
        zt: ZtClient::new(cfg.zt_port, cfg.zt_home.join("authtoken.secret")),
        store: Arc::new(Mutex::new(store)),
        cfg: cfg.clone(),
    };

    // 面板：路由按根路径定义，外层去掉 BASE_PATH 前缀。
    // 反向代理无论是否剥离前缀（proxy_pass 带不带尾部 /）都能工作
    let base = cfg.base_path.clone();
    let panel = api::router().fallback(static_asset).with_state(state);
    let base_for_strip = base.clone();
    let panel = tower::util::MapRequestLayer::new(move |req: Request| strip_base(req, &base_for_strip)).layer(panel);

    let files = files_router(cfg.clone(), key);

    let panel_listener = tokio::net::TcpListener::bind(("0.0.0.0", cfg.api_port)).await?;
    let file_listener = tokio::net::TcpListener::bind(("0.0.0.0", cfg.file_port)).await?;
    println!("面板监听 :{}{}/ ，文件服务监听 :{}", cfg.api_port, base, cfg.file_port);

    let panel_srv = axum::serve(panel_listener, ServiceExt::<Request>::into_make_service(panel)).with_graceful_shutdown(shutdown());
    let file_srv = axum::serve(file_listener, files).with_graceful_shutdown(shutdown());
    let (a, b) = tokio::join!(panel_srv, file_srv);
    a?;
    b?;
    Ok(())
}

fn files_router(cfg: Arc<Config>, key: Arc<String>) -> Router {
    Router::new().fallback(file_server).with_state((cfg, key))
}

/// 仅运行文件服务（MODE=root 的从根节点使用，不运行面板）
async fn serve_files(cfg: Config) -> AppResult<()> {
    let key = Arc::new(file_key(&cfg)?);
    let cfg = Arc::new(cfg);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", cfg.file_port)).await?;
    println!("文件服务监听 :{}", cfg.file_port);
    axum::serve(listener, files_router(cfg, key)).with_graceful_shutdown(shutdown()).await?;
    Ok(())
}

async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

fn strip_base(mut req: Request, base: &str) -> Request {
    if base.is_empty() {
        return req;
    }
    let path = req.uri().path();
    let rest = match path.strip_prefix(base) {
        Some("") => {
            // /base -> 交给 static_asset 重定向到 /base/
            "/__redirect_base__".to_string()
        }
        Some(r) if r.starts_with('/') => r.to_string(),
        _ => return req,
    };
    let pq = match req.uri().query() {
        Some(q) => format!("{rest}?{q}"),
        None => rest,
    };
    if let Ok(uri) = pq.parse::<Uri>() {
        *req.uri_mut() = uri;
    }
    req
}

async fn static_asset(State(st): State<AppState>, uri: Uri) -> Response {
    let base = &st.cfg.base_path;
    let path = uri.path();
    if path == "/__redirect_base__" {
        return Redirect::permanent(&format!("{base}/")).into_response();
    }
    if path.starts_with("/api/") {
        return AppError::not_found().into_response();
    }
    let name = path.trim_start_matches('/');
    let name = if name.is_empty() { "index.html" } else { name };
    let Some(file) = Assets::get(name) else {
        return (StatusCode::NOT_FOUND, "404").into_response();
    };
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let mut body = file.data.into_owned();
    if name == "index.html" {
        // 注入 <base href>，前端统一使用相对路径，从而支持任意 BASE_PATH
        body = String::from_utf8_lossy(&body).replace("__BASE__", &format!("{base}/")).into_bytes();
    }
    let cache = if name == "index.html" { "no-cache" } else { "public, max-age=3600" };
    Response::builder()
        .header(header::CONTENT_TYPE, HeaderValue::from_str(mime.as_ref()).unwrap_or(HeaderValue::from_static("application/octet-stream")))
        .header(header::CACHE_CONTROL, cache)
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from(body))
        .unwrap()
}

/// 独立文件服务：GET /<文件名>?key=<密钥>，供客户端直接下载 planet / moon
async fn file_server(
    State((cfg, key)): State<(Arc<Config>, Arc<String>)>,
    uri: Uri,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let ok = q.get("key").map(|k| auth::ct_eq(k, &key)).unwrap_or(false);
    if !ok {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    match api::file_response(&cfg, uri.path().trim_start_matches('/')) {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}
