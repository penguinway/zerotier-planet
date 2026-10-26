#!/bin/sh
# ZeroTier Planet 容器入口
#   MODE=controller（默认）：根节点 + 网络控制器 + 管理面板 + 文件服务
#   MODE=root：仅作为附加根节点（多根 planet），从主节点同步 planet
set -eu

ZT_HOME=${ZT_HOME:-/var/lib/zerotier-one}
CONFIG_DIR=${CONFIG_DIR:-/app/config}
DIST_DIR=${DIST_DIR:-/app/dist}
MODE=${MODE:-controller}
ZT_PORT=${ZT_PORT:-9994}
PLANET_URL=${PLANET_URL:-}
IP_ADDR4=${IP_ADDR4:-}
IP_ADDR6=${IP_ADDR6:-}
export ZT_HOME CONFIG_DIR DIST_DIR

log() { echo "[entrypoint] $*"; }

mkdir -p "$ZT_HOME" "$CONFIG_DIR" "$DIST_DIR"

# ---------- 端口：首次启动写入 config，之后以 config 为准（与 deploy.sh 一致）
if [ -s "$CONFIG_DIR/zerotier-one.port" ]; then
    ZT_PORT=$(cat "$CONFIG_DIR/zerotier-one.port")
else
    echo "$ZT_PORT" > "$CONFIG_DIR/zerotier-one.port"
fi
export ZT_PORT

# ---------- 节点身份
if [ ! -s "$ZT_HOME/identity.secret" ]; then
    log "生成节点身份"
    zerotier-idtool generate "$ZT_HOME/identity.secret" "$ZT_HOME/identity.public" >/dev/null
fi
[ -s "$ZT_HOME/identity.public" ] || zerotier-idtool getpublic "$ZT_HOME/identity.secret" > "$ZT_HOME/identity.public"
if [ ! -s "$ZT_HOME/authtoken.secret" ]; then
    openssl rand -hex 16 | tr -d '\n' > "$ZT_HOME/authtoken.secret"
fi
chmod 600 "$ZT_HOME/identity.secret" "$ZT_HOME/authtoken.secret"
NODE_ID=$(cut -d: -f1 "$ZT_HOME/identity.public")
log "节点地址 $NODE_ID，ZeroTier $(zerotier-one -v)"

# ---------- 公网地址：环境变量 > 已保存 > 自动探测
detect_ip() {
    # $1: 4|6
    curl -"$1" -fsS --max-time 5 "https://ipv$1.icanhazip.com/" 2>/dev/null | tr -d ' \n' || true
}
IP_ADDR4=${IP_ADDR4:-$(cat "$CONFIG_DIR/ip_addr4" 2>/dev/null || true)}
IP_ADDR6=${IP_ADDR6:-$(cat "$CONFIG_DIR/ip_addr6" 2>/dev/null || true)}
if [ -z "$IP_ADDR4" ] && [ -z "$IP_ADDR6" ]; then
    log "未指定 IP_ADDR4/IP_ADDR6，自动探测公网地址"
    IP_ADDR4=$(detect_ip 4)
    IP_ADDR6=$(detect_ip 6)
fi
echo "$IP_ADDR4" > "$CONFIG_DIR/ip_addr4"
echo "$IP_ADDR6" > "$CONFIG_DIR/ip_addr6"
export IP_ADDR4 IP_ADDR6

ENDPOINTS=$(jq -cn --arg v4 "$IP_ADDR4" --arg v6 "$IP_ADDR6" --arg p "$ZT_PORT" \
    '[$v4, $v6] | map(select(. != "")) | map(. + "/" + $p)')
if [ "$ENDPOINTS" = "[]" ]; then
    log "错误：无法获取公网 IP，请设置 IP_ADDR4 或 IP_ADDR6"
    exit 1
fi
log "本机端点 $ENDPOINTS"

# 供其他根节点 / 主节点导入的本机根节点信息
jq -n --arg id "$(cat "$ZT_HOME/identity.public")" --argjson eps "$ENDPOINTS" --arg name "${ROOT_NAME:-$NODE_ID}" \
    '{name: $name, identity: $id, stableEndpoints: $eps}' > "$DIST_DIR/root.json"

# 从旧版（docker-zerotier-planet）迁移：沿用其 moon.json 中的端点，保证已有 planet 不变
LEGACY_EPS=""
if [ ! -s "$CONFIG_DIR/planet.json" ] && [ -s "$ZT_HOME/moon.json" ]; then
    LEGACY_EPS=$(jq -c '.roots[0].stableEndpoints // []' "$ZT_HOME/moon.json" 2>/dev/null || echo '[]')
fi

# ---------- moon（兼容旧客户端的 orbit 用法）
if ! ls "$ZT_HOME"/moons.d/*.moon >/dev/null 2>&1; then
    log "生成 moon"
    (
        cd "$ZT_HOME"
        zerotier-idtool initmoon identity.public > moon.json
        jq --argjson eps "$ENDPOINTS" '.roots[0].stableEndpoints = $eps' moon.json > moon.json.tmp && mv moon.json.tmp moon.json
        zerotier-idtool genmoon moon.json >/dev/null
        mkdir -p moons.d && mv ./*.moon moons.d/
    )
fi
rm -f "$DIST_DIR"/*.moon
cp "$ZT_HOME"/moons.d/*.moon "$DIST_DIR"/

# ---------- zerotier-one 守护：被面板 kill（重新生成 planet）后自动重启
ZT_PID=""
SUPERVISOR_PID=""
supervise() {
    while true; do
        zerotier-one -p"$ZT_PORT" "$ZT_HOME" &
        ZT_PID=$!
        wait "$ZT_PID" || true
        log "zerotier-one 已退出，1 秒后重启"
        sleep 1
    done
}

stop_all() {
    log "正在停止"
    [ -n "$SUPERVISOR_PID" ] && kill "$SUPERVISOR_PID" 2>/dev/null || true
    pkill -TERM -x zerotier-one 2>/dev/null || true
    [ -n "${APP_PID:-}" ] && kill -TERM "$APP_PID" 2>/dev/null || true
    wait 2>/dev/null || true
    exit 0
}
trap stop_all TERM INT

wait_zt() {
    i=0
    while [ $i -lt 30 ]; do
        if curl -fsS --max-time 2 -H "X-ZT1-Auth: $(cat "$ZT_HOME/authtoken.secret")" \
            "http://127.0.0.1:$ZT_PORT/status" >/dev/null 2>&1; then
            return 0
        fi
        i=$((i + 1))
        sleep 1
    done
    log "警告：zerotier-one 30 秒内未就绪"
}

install_planet() {
    # $1: planet 文件
    cp "$1" "$ZT_HOME/planet.tmp" && mv "$ZT_HOME/planet.tmp" "$ZT_HOME/planet"
    cp "$1" "$DIST_DIR/planet.tmp" && mv "$DIST_DIR/planet.tmp" "$DIST_DIR/planet"
}

# ========== 附加根节点 ==========
if [ "$MODE" = "root" ]; then
    log "以附加根节点模式运行"
    log "请将以下内容粘贴到主节点面板「Planet → 导入根节点」："
    cat "$DIST_DIR/root.json"

    sync_planet() {
        [ -n "$PLANET_URL" ] || return 0
        if curl -fsSL --max-time 15 -o /tmp/planet.new "$PLANET_URL"; then
            if ! cmp -s /tmp/planet.new "$ZT_HOME/planet"; then
                log "planet 已更新"
                install_planet /tmp/planet.new
                return 10
            fi
        else
            log "警告：从 PLANET_URL 下载 planet 失败"
        fi
        return 0
    }

    if [ -z "$PLANET_URL" ]; then
        log "警告：未设置 PLANET_URL，不会从主节点同步 planet"
    fi
    sync_planet || true

    supervise &
    SUPERVISOR_PID=$!
    ztpanel files &
    APP_PID=$!

    # 定期同步 planet，变化后重启 zerotier-one 以加载
    while true; do
        sleep 600 &
        wait $! || true
        rc=0
        sync_planet || rc=$?
        if [ "$rc" = 10 ]; then
            pkill -TERM -x zerotier-one || true
        fi
    done
fi

# ========== 控制器 ==========
PLANET_JSON="$CONFIG_DIR/planet.json"
if [ ! -s "$PLANET_JSON" ]; then
    EPS="$ENDPOINTS"
    if [ -n "$LEGACY_EPS" ] && [ "$LEGACY_EPS" != "[]" ]; then
        EPS="$LEGACY_EPS"
        log "沿用旧版 moon.json 中的端点 $EPS"
    fi
    jq -n --arg id "$(cat "$ZT_HOME/identity.public")" --argjson eps "$EPS" --arg name "${ROOT_NAME:-$NODE_ID}" \
        '{roots: [{name: $name, identity: $id, stableEndpoints: $eps}]}' > "$PLANET_JSON"
    log "已创建 $PLANET_JSON"
fi

# planet.json 有更新或 planet 不存在时重新生成（签名密钥保存在 $ZT_HOME，保持不变）
if [ ! -s "$DIST_DIR/planet" ] || [ ! -s "$ZT_HOME/planet" ] || [ "$PLANET_JSON" -nt "$DIST_DIR/planet" ]; then
    log "生成 planet"
    ztpanel planet-build
fi

supervise &
SUPERVISOR_PID=$!
wait_zt

ztpanel serve &
APP_PID=$!
wait "$APP_PID" || true
stop_all
