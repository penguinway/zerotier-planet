#!/bin/bash
# ZeroTier Planet 一键部署脚本
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CONTAINER_NAME="${CONTAINER_NAME:-ztplanet}"
DATA_PATH="${DATA_PATH:-${SCRIPT_DIR}/data/zerotier}"
ENV_FILE="${DATA_PATH}/deploy.env"
IMAGE="${IMAGE:-zerotier-planet:latest}"

msg() { echo -e "\033[${2:-32}m$1\033[0m"; }
die() { msg "$1" 31; exit 1; }

command -v docker >/dev/null || die "未安装 docker，请先安装：curl -fsSL https://get.docker.com | sh"

port_in_use() {
    if command -v ss >/dev/null; then
        ss -lntuH 2>/dev/null | awk '{print $5}' | grep -Eq "[:.]$1\$"
    elif command -v lsof >/dev/null; then
        lsof -i:"$1" >/dev/null 2>&1
    else
        return 1
    fi
}

read_port() {
    local prompt=$1 def=$2 port
    while :; do
        read -rp "${prompt} [${def}]: " port
        port=${port:-$def}
        if ! [[ "$port" =~ ^[0-9]+$ ]] || ((port < 1 || port > 65535)); then
            echo "端口必须是 1-65535 的数字" >&2
        elif port_in_use "$port"; then
            echo "端口 ${port} 已被占用" >&2
        else
            echo "$port"
            return
        fi
    done
}

read_ips() {
    local v4 v6 ans
    v4=$(curl -4 -fsS --max-time 5 https://ipv4.icanhazip.com/ 2>/dev/null | tr -d '\n' || true)
    v6=$(curl -6 -fsS --max-time 5 https://ipv6.icanhazip.com/ 2>/dev/null | tr -d '\n' || true)
    echo "探测到 IPv4: ${v4:-无}  IPv6: ${v6:-无}"
    read -rp "使用探测到的地址？(Y/n) " ans
    if [[ "$ans" =~ ^[Nn]$ ]]; then
        read -rp "IPv4 地址（可留空）: " v4
        read -rp "IPv6 地址（可留空）: " v6
    fi
    [ -n "$v4$v6" ] || die "IPv4 与 IPv6 不能同时为空"
    IP_ADDR4=$v4
    IP_ADDR6=$v6
}

ensure_image() {
    if [ "${1:-}" = "rebuild" ] || ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
        if [ -f "${SCRIPT_DIR}/Dockerfile" ] && [[ "$IMAGE" == zerotier-planet:* ]]; then
            msg "构建镜像 ${IMAGE}（首次构建需要几分钟）..."
            docker build -t "$IMAGE" "$SCRIPT_DIR"
        else
            docker pull "$IMAGE"
        fi
    fi
}

save_env() {
    mkdir -p "$DATA_PATH"
    cat > "$ENV_FILE" <<EOF
MODE=${MODE}
ZT_PORT=${ZT_PORT}
API_PORT=${API_PORT}
FILE_SERVER_PORT=${FILE_SERVER_PORT}
IP_ADDR4=${IP_ADDR4}
IP_ADDR6=${IP_ADDR6}
BASE_PATH=${BASE_PATH}
PLANET_URL=${PLANET_URL}
ROOT_NAME=${ROOT_NAME}
EOF
    chmod 600 "$ENV_FILE"
}

load_env() {
    [ -f "$ENV_FILE" ] || die "未找到 ${ENV_FILE}，请先安装"
    # shellcheck disable=SC1090
    . "$ENV_FILE"
}

run_container() {
    local ports=(-p "${ZT_PORT}:${ZT_PORT}/udp" -p "${ZT_PORT}:${ZT_PORT}/tcp" -p "${FILE_SERVER_PORT}:${FILE_SERVER_PORT}")
    [ "$MODE" = "controller" ] && ports+=(-p "${API_PORT}:${API_PORT}")
    docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
    docker run -d --name "$CONTAINER_NAME" --restart unless-stopped \
        "${ports[@]}" \
        --env-file "$ENV_FILE" \
        -v "${DATA_PATH}/one:/var/lib/zerotier-one" \
        -v "${DATA_PATH}/config:/app/config" \
        -v "${DATA_PATH}/dist:/app/dist" \
        "$IMAGE" >/dev/null
}

wait_ready() {
    local i
    for i in $(seq 1 30); do
        [ -s "${DATA_PATH}/config/file_server.key" ] && [ -s "${DATA_PATH}/dist/root.json" ] && return 0
        sleep 1
    done
    msg "容器启动较慢，请用 docker logs ${CONTAINER_NAME} 查看日志" 33
}

host_for_url() {
    if [ -n "$IP_ADDR4" ]; then echo "$IP_ADDR4"; else echo "[$IP_ADDR6]"; fi
}

info() {
    load_env
    local host key
    host=$(host_for_url)
    key=$(cat "${DATA_PATH}/config/file_server.key" 2>/dev/null || true)
    echo "---------------------------------------------"
    if [ "$MODE" = "controller" ]; then
        msg "管理面板: http://${host}:${API_PORT}${BASE_PATH}/"
        echo "首次登录: admin / password（登录后需修改密码）"
        echo "planet 下载: http://${host}:${FILE_SERVER_PORT}/planet?key=${key}"
        for f in "${DATA_PATH}"/dist/*.moon; do
            [ -e "$f" ] && echo "moon 下载:   http://${host}:${FILE_SERVER_PORT}/$(basename "$f")?key=${key}"
        done
        echo "请放行端口: ${ZT_PORT}/udp ${ZT_PORT}/tcp ${API_PORT}/tcp ${FILE_SERVER_PORT}/tcp"
    else
        msg "附加根节点运行中，请把以下内容粘贴到主节点面板「Planet → 导入根节点」:"
        cat "${DATA_PATH}/dist/root.json" 2>/dev/null || echo "（尚未生成，请稍后重试）"
        echo
        echo "请放行端口: ${ZT_PORT}/udp ${ZT_PORT}/tcp ${FILE_SERVER_PORT}/tcp"
    fi
    echo "数据目录: ${DATA_PATH}"
    echo "---------------------------------------------"
}

confirm_overwrite() {
    if docker inspect "$CONTAINER_NAME" >/dev/null 2>&1 || [ -d "${DATA_PATH}/one" ]; then
        msg "已存在安装（容器 ${CONTAINER_NAME} 或数据目录 ${DATA_PATH}）" 33
        read -rp "删除旧数据并重新安装？所有网络和身份将丢失！输入 yes 确认: " ans
        [ "$ans" = "yes" ] || die "已取消；如需升级请选择「更新」"
        docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
        rm -rf "$DATA_PATH"
    fi
}

install_controller() {
    confirm_overwrite
    MODE=controller
    ZT_PORT=$(read_port "ZeroTier 端口" 9994)
    API_PORT=$(read_port "管理面板端口" 3443)
    FILE_SERVER_PORT=$(read_port "文件下载端口" 3000)
    read_ips
    read -rp "面板子路径 BASE_PATH（反向代理用，例如 /zerotier，可留空）: " BASE_PATH
    BASE_PATH=${BASE_PATH//[[:space:]]/}
    PLANET_URL=""
    ROOT_NAME=""
    save_env
    ensure_image
    run_container
    wait_ready
    msg "安装完成"
    info
}

install_root() {
    confirm_overwrite
    MODE=root
    ZT_PORT=$(read_port "ZeroTier 端口" 9994)
    FILE_SERVER_PORT=$(read_port "文件下载端口" 3000)
    API_PORT=3443
    read_ips
    read -rp "主节点 planet 下载地址（主节点面板首页可复制，用于自动同步）: " PLANET_URL
    read -rp "根节点名称（例如 香港，可留空）: " ROOT_NAME
    ROOT_NAME=${ROOT_NAME//[[:space:]]/_}
    BASE_PATH=""
    save_env
    ensure_image
    run_container
    wait_ready
    msg "附加根节点已启动"
    info
}

update() {
    load_env
    ensure_image rebuild
    run_container
    msg "已更新并重启，数据保留"
    info
}

reset_password() {
    load_env
    [ "$MODE" = "controller" ] || die "附加根节点没有管理面板"
    local user
    read -rp "要重置的用户 [admin]: " user
    docker exec "$CONTAINER_NAME" ztpanel reset-password "${user:-admin}"
    docker restart "$CONTAINER_NAME" >/dev/null
    msg "已重启容器，新密码已生效"
}

uninstall() {
    read -rp "确认卸载？(y/N) " ans
    [[ "$ans" =~ ^[Yy]$ ]] || exit 0
    docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
    read -rp "同时删除数据目录 ${DATA_PATH}？(y/N) " ans
    [[ "$ans" =~ ^[Yy]$ ]] && rm -rf "$DATA_PATH"
    msg "已卸载"
}

menu() {
    cat <<EOF
==============================================
  ZeroTier Planet（ZeroTier + 自建 planet + 管理面板）
==============================================
  1. 安装主节点（控制器 + 管理面板）
  2. 安装附加根节点（多根 planet）
  3. 更新（保留数据）
  4. 查看信息
  5. 重置面板密码
  6. 卸载
  0. 退出
EOF
    read -rp "请选择: " choice
    case "$choice" in
        1) install_controller ;;
        2) install_root ;;
        3) update ;;
        4) info ;;
        5) reset_password ;;
        6) uninstall ;;
        *) exit 0 ;;
    esac
}

case "${1:-}" in
    install) install_controller ;;
    install-root) install_root ;;
    update) update ;;
    info) info ;;
    reset-password) reset_password ;;
    uninstall) uninstall ;;
    *) menu ;;
esac
