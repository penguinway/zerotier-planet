#!/bin/bash
# 构建多架构镜像并推送。默认使用 ZeroTier 最新稳定版
set -e

DOCKERHUB_USERNAME="${DOCKERHUB_USERNAME:?请设置 DOCKERHUB_USERNAME}"
DOCKER_IMAGE="${DOCKERHUB_USERNAME}/zerotier-planet"

if [ -n "${SPEC_ZEROTIER_TAG}" ]; then
    TAG="${SPEC_ZEROTIER_TAG}"
else
    TAG=$(curl -fsS "https://api.github.com/repos/zerotier/ZeroTierOne/releases/latest" | jq -r .tag_name)
fi
[ -n "$TAG" ] && [ "$TAG" != "null" ] || { echo "获取 ZeroTier 版本失败"; exit 1; }
echo "ZeroTier 版本: ${TAG}"

if [ "${FORCE_REBUILD}" != "1" ] && docker manifest inspect "${DOCKER_IMAGE}:${TAG}" >/dev/null 2>&1; then
    echo "${DOCKER_IMAGE}:${TAG} 已存在，跳过。设置 FORCE_REBUILD=1 可强制重建"
    exit 0
fi

docker buildx build \
    --platform "${PLATFORMS:-linux/amd64,linux/arm64}" \
    --push \
    --build-arg TAG="${TAG}" \
    -t "${DOCKER_IMAGE}:latest" \
    -t "${DOCKER_IMAGE}:${TAG}" \
    .
