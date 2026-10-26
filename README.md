# ZeroTier Planet

一键部署私有 ZeroTier 根服务器（planet）+ 网络控制器 + 自带管理面板。

参考 [piwind/docker-zerotier-planet](https://github.com/piwind/docker-zerotier-planet) 重写，用 Rust 编写的轻量面板取代 ztncui，并完成了原项目的全部 TODO。

- ZeroTier 默认 **1.16.2**（构建参数 `TAG` 可指定版本，CI 每周自动跟进官方最新版），控制器功能完整
- 管理面板为单个 Rust 静态二进制（约 2 MB），整个镜像约 37 MB，不含 Node.js
- 面板与 planet 文件下载服务内置在同一进程中

## 原项目 TODO 完成情况

| TODO | 实现 |
| --- | --- |
| 多 planet 支持 | 「Planet」页面管理最多 4 个根节点；附加服务器以 `MODE=root` 运行，导入后重新签名生成 planet，已有客户端**自动更新**，无需重新分发 |
| 【BUG】IP assignment 重复添加相同 IP | 后端对 IP 列表做规范化去重（IPv6 的不同写法也视为同一地址），前端请求进行中禁用按钮并预先查重 |
| 只有 admin 能删除用户、用户间网络隔离 | 管理员 / 普通用户两种角色。普通用户只能看到和管理自己的网络；只有管理员可管理用户、planet、系统设置、转移网络所有者；不能删除自己或最后一个管理员 |
| 编辑 rule 的功能 | 「规则」页使用官方 rule-compiler（与 ZeroTier Central 同一份）在浏览器中编译，支持 capability / tag，显示出错行列，保存源码便于再次编辑 |
| 二维码 | 网络 ID、planet 下载地址、完整接入信息均可扫码 |
| 手机版界面不友好 | 移动优先的响应式界面：底部标签栏、成员卡片、表格自动变为卡片布局，支持深色模式 |

## 与其他管理面板的比较

- **本项目自带面板**：专为自建 planet 设计（planet / 多根节点 / 文件下载 / 二维码一体化），轻量，不依赖数据库。
- **[ztnet](https://github.com/sinamics/ztnet)**：目前功能最完整的开源 ZeroTier 控制器面板（Next.js + PostgreSQL）。支持多用户、组织、规则编辑、网络流量统计，也内置了私有 planet 生成，界面现代且适配手机。需要更多团队协作功能时推荐。
- **[ZeroUI](https://github.com/dec0dOS/zero-ui)**：简洁的 React 面板，接近 Central 的体验，但多用户能力较弱，更新较慢。

## 快速开始

需要一台有公网 IP 的服务器和 Docker。

```bash
git clone https://github.com/penguinway/zerotier-planet.git && cd zerotier-planet
./deploy.sh          # 选择 1 安装主节点
```

或使用 docker compose：

```bash
docker compose up -d --build
```

安装后访问 `http://服务器IP:3443`，初始账号 `admin` / `password`，首次登录必须修改密码（也可通过 `ADMIN_PASSWORD` 环境变量直接指定）。

需放行端口：

| 端口 | 用途 |
| --- | --- |
| 9994/udp、9994/tcp | ZeroTier 通信 |
| 3443/tcp | 管理面板 |
| 3000/tcp | planet / moon 文件下载 |

## 客户端接入

面板「概览」页和每个网络的「加入 / 二维码」页都提供了可复制的命令。以 Linux 为例：

```bash
curl -fsSL -o /var/lib/zerotier-one/planet "http://服务器IP:3000/planet?key=xxxx"
systemctl restart zerotier-one
zerotier-cli join <网络ID>
```

| 系统 | planet 文件位置 |
| --- | --- |
| Linux | `/var/lib/zerotier-one/planet` |
| Windows | `C:\ProgramData\ZeroTier\One\planet` |
| macOS | `/Library/Application Support/ZeroTier/One/planet` |
| Android | 官方 App 不支持；可用 ZerotierFix 等支持自定义 planet 的客户端，扫码填入 planet 地址和网络 ID |

然后在面板中授权设备即可。

## 多根节点（多 planet）

一个 planet 可包含最多 4 个根节点。多个根节点互为备份，单台服务器故障不影响已建立的网络。

1. 在另一台服务器上运行 `./deploy.sh`，选择 **2 安装附加根节点**，填入主节点的 planet 下载地址（面板首页可复制）。
2. 安装完成后会输出一段 `root.json`（也保存在 `data/zerotier/dist/root.json`）。
3. 在主节点面板「Planet → 导入根节点」粘贴并保存。主节点会重新签名生成 planet 并重启 zerotier-one。
4. 已使用旧 planet 的客户端会从根节点自动获取新 planet；附加根节点每 10 分钟从主节点同步一次 planet。

> 原理：planet 由保存在主节点 `data/zerotier/one/*.c25519` 的密钥签名。只要密钥不变，时间戳更新的 planet 会被客户端自动接受。**请务必备份 `data/zerotier/one` 目录。**

在面板「Planet」页也可以修改每个根节点的端点（例如更换 IP 或端口、增加 IPv6），保存后同样会自动下发。

## 从 docker-zerotier-planet 迁移

旧项目的 `data/zerotier` 下有 `one`（节点身份、网络、planet 签名密钥）和 `config`（端口、IP、文件下载密钥）两个目录，**两个都要复制**：

1. 停止旧容器并备份 `data/zerotier`。
2. 运行 `./deploy.sh` 安装主节点，端口填写与旧部署相同的值（例如 9993、3443、3000）。
3. `docker stop ztplanet`，用旧的 `one`、`config` 覆盖新的 `data/zerotier/one`、`data/zerotier/config`，再 `docker start ztplanet`。

> `deploy.sh install` 会删除已存在的数据目录，所以必须先安装、再覆盖，不要先把旧数据放进去。使用 docker compose 时直接挂载旧目录，并把 `ZT_PORT` 和端口映射设为旧端口即可。

迁移后：

- 节点身份、网络、成员、planet 签名密钥、端口和文件下载密钥都会保留，客户端无需任何修改；
- 首次启动时会沿用旧 `moon.json` 中的端点生成 `config/planet.json`，planet ID 不变；
- ztncui 的账号不会迁移，使用 `admin` / `password` 登录后修改密码，并在各网络的「设置」页为原有网络分配所有者（管理员默认可见全部网络）。

## 环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `MODE` | `controller` | `controller`：主节点；`root`：附加根节点 |
| `IP_ADDR4` / `IP_ADDR6` | 自动探测 | 写入 planet 的公网地址 |
| `ZT_PORT` | `9994` | ZeroTier 端口（首次启动后以 `config/zerotier-one.port` 为准） |
| `API_PORT` | `3443` | 面板端口 |
| `FILE_SERVER_PORT` | `3000` | 文件下载端口 |
| `BASE_PATH` | 空 | 面板子路径，例如 `/zerotier` |
| `ADMIN_PASSWORD` | 空 | 首次启动时的管理员密码；为空则为 `password` 且强制修改 |
| `FILE_KEY` | 自动生成 | 文件下载密钥，生成后保存在 `config/file_server.key` |
| `PLANET_URL` | 空 | `MODE=root` 时从主节点同步 planet 的地址 |
| `ROOT_NAME` | 节点地址 | 根节点显示名称 |

## 反向代理（Nginx）

```nginx
location /zerotier/ {
    proxy_pass http://127.0.0.1:3443;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-Proto $scheme;
}
location /zerotier-files/ {
    proxy_pass http://127.0.0.1:3000/;
}
```

设置 `BASE_PATH=/zerotier`，并在面板「设置」中填写文件服务对外地址 `https://example.com/zerotier-files`，二维码和下载链接会使用该地址。`proxy_pass` 带不带 URI 后缀均可。

## 运维

```bash
./deploy.sh info             # 查看访问地址和下载链接
./deploy.sh update           # 重新构建镜像并重启，保留数据
./deploy.sh reset-password   # 忘记密码时重置为 password
docker logs ztplanet         # 查看日志
```

## 开发

```
Dockerfile          # 三阶段：编译 zerotier-one + mkworld、编译面板、运行镜像
docker/entrypoint.sh
mkworld/mkworld.cpp # 独立的 planet 生成工具（1.16 起官方移除了 attic/world）
panel/              # Rust 面板（axum），前端为无构建步骤的原生 JS，内嵌进二进制
```

```bash
cd panel && cargo test
```

### 发布镜像（GitHub Actions）

在仓库 Secrets 中配置 `DOCKERHUB_USERNAME`、`DOCKERHUB_TOKEN`。

- 普通 push / PR：只运行测试和 clippy
- 推送 `v*` tag、在 Actions 页手动运行：构建并发布 `latest`、`<ZeroTier 版本>`（以及 `<tag>`）
- 每周一自动检查 ZeroTier 新版本，镜像不存在时才构建

amd64 与 arm64 分别在原生 runner 上并行构建（不使用 QEMU），并启用构建缓存。

## 许可

- 本项目代码：[MIT](LICENSE)（vendor 中的第三方文件除外）
- ZeroTier 1.16 起，网络控制器代码位于 `nonfree/` 目录，使用 [源码可用的非商业许可](https://github.com/zerotier/ZeroTierOne/blob/main/nonfree/LICENSE.md)，镜像构建时启用了 `ZT_NONFREE=1`。**商业用途请先阅读该许可或联系 ZeroTier 获取授权。**
- `panel/public/vendor/rule-compiler.js`：ZeroTier 官方规则编译器，MPL-2.0
- `panel/public/vendor/qrcode.js`：qrcode-generator，MIT
