# ---------- 构建 zerotier-one 与 mkworld ----------
FROM alpine:3.22 AS builder

# ZeroTier 版本（git tag），默认使用当前最新稳定版
ARG TAG=1.16.2

RUN apk add --no-cache build-base linux-headers git curl openssl-dev pkgconfig

WORKDIR /src
RUN git clone --depth 1 --branch ${TAG} https://github.com/zerotier/ZeroTierOne.git

# ZT_NONFREE=1：1.16 起控制器代码移入 nonfree/，不加该参数构建出的程序不含 controller
# ZT_SSO_SUPPORTED=0：自建控制器不需要 OIDC，跳过 Rust 依赖，加快构建
RUN cd ZeroTierOne \
    && make -j"$(nproc)" ZT_NONFREE=1 ZT_SSO_SUPPORTED=0 one \
    && strip zerotier-one

# 1.16 起 attic/world 已移除，这里用独立的 mkworld 源码编译
COPY mkworld/mkworld.cpp /src/mkworld/mkworld.cpp
RUN cd ZeroTierOne \
    && KEYSRC=$( [ -f node/ECC.cpp ] && echo node/ECC.cpp || echo node/C25519.cpp ) \
    && AESSRC=$(ls node/AES*.cpp 2>/dev/null | tr '\n' ' ') \
    && g++ -std=c++17 -O2 -I. -Iext -o /src/mkworld/mkworld \
        $KEYSRC $AESSRC node/Salsa20.cpp node/SHA512.cpp node/Identity.cpp node/Utils.cpp \
        node/InetAddress.cpp osdep/OSUtils.cpp /src/mkworld/mkworld.cpp -lm \
    && strip /src/mkworld/mkworld

# ---------- 构建管理面板（Rust，静态链接 musl） ----------
FROM rust:1-alpine AS panel
RUN apk add --no-cache musl-dev
WORKDIR /src/panel
# 先只拷贝清单以缓存依赖编译
COPY panel/Cargo.toml panel/Cargo.lock* ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo build --release && rm -rf src
COPY panel/ ./
RUN touch src/main.rs && cargo build --release && cp target/release/ztpanel /ztpanel

# ---------- 运行镜像 ----------
FROM alpine:3.22

ENV TZ=Asia/Shanghai \
    MODE=controller \
    IP_ADDR4='' \
    IP_ADDR6='' \
    ZT_PORT=9994 \
    API_PORT=3443 \
    FILE_SERVER_PORT=3000 \
    BASE_PATH='' \
    FILE_KEY='' \
    ADMIN_PASSWORD='' \
    PLANET_URL=''

RUN apk add --no-cache libstdc++ libgcc openssl curl jq tzdata

COPY --from=builder /src/ZeroTierOne/zerotier-one /usr/sbin/zerotier-one
COPY --from=builder /src/mkworld/mkworld /usr/local/bin/mkworld
RUN ln -s /usr/sbin/zerotier-one /usr/sbin/zerotier-idtool \
    && ln -s /usr/sbin/zerotier-one /usr/sbin/zerotier-cli

# 面板为单个静态二进制，前端资源（含官方 rule-compiler.js）已内嵌
COPY --from=panel /ztpanel /usr/local/bin/ztpanel
COPY docker/entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh && mkdir -p /app/config /app/dist /var/lib/zerotier-one

VOLUME ["/app/dist", "/app/config", "/var/lib/zerotier-one"]
EXPOSE 9994/udp 9994/tcp 3443/tcp 3000/tcp

CMD ["/app/entrypoint.sh"]
