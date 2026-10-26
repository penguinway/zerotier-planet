# CLAUDE.md

私有 ZeroTier 根服务器（planet）+ 网络控制器 + Rust 管理面板的一体化 Docker 镜像，重写自 piwind/docker-zerotier-planet。用户文档见 README.md。

## 运行与测试（本机没有 Rust/Go，全部走 Docker）

```bash
# 面板单元测试 / clippy（在 panel/ 下执行）
docker run --rm -v ztp-cargo:/usr/local/cargo/registry -v "$PWD":/w -w /w \
  -e CARGO_TARGET_DIR=/w/target rust:1-alpine \
  sh -c 'apk add -q musl-dev; cargo test && rustup component add clippy && cargo clippy --all-targets -- -D warnings'

# 构建并运行镜像（仓库根目录）
docker build -t ztplanet:dev .
docker run -d --name ztp -e IP_ADDR4=127.0.0.1 -p 3443:3443 -p 3000:3000 ztplanet:dev

# 检查 workflow
docker run --rm -v "$PWD":/repo -w /repo rhysd/actionlint:latest -no-color
```

## 技术栈

- ZeroTier 1.16.x 源码编译，`make ZT_NONFREE=1 ZT_SSO_SUPPORTED=0`（控制器在 nonfree/，非商业许可）
- `mkworld/mkworld.cpp`：独立 planet 生成器（1.16 起官方删除了 attic/world；C25519 改名为 ECC）
- `panel/`：Rust（axum 0.8、tokio、reqwest、argon2），用 rust-embed 把 `panel/public/` 嵌入二进制
- 前端：原生 JS 单页应用（`app.js`，hash 路由），没有构建步骤；`vendor/` 里是官方 rule-compiler 和 qrcode-generator
- `docker/entrypoint.sh`：POSIX sh；`MODE=controller|root`

## 约定

- git：仓库级身份 penguinway，用 `~/.ssh/id_rsa`，直接提交到 main
- ztpanel 子命令：`serve`、`files`、`planet-build`、`reset-password`
- planet 签名密钥（`one/previous|current.c25519`）和 world ID 149604618 不能改，否则已有客户端失联
- `/api/settings`：GET 返回 snake_case，PATCH 接收 camelCase
- 网络规则源码由面板自己保存；ZeroTier 只存编译后的规则
- 判断“本机根节点”读 `identity.public`（`planet::local_address`），不调用 ZT API，因为重建 planet 时 zerotier-one 会重启
- 镜像发布：只有 `v*` tag、手动触发和每周定时会构建；amd64 和 arm64 在原生 runner 上构建，不用 QEMU

## 状态

- 原项目 TODO 全部完成，端到端测试和旧版数据迁移模拟都已通过（2026-10）
- CI：e6f998f 的手动发布（workflow_dispatch）已成功
- 下一步：无硬性待办；需要时打 `v*` tag 发版
