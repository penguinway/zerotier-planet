/*
 * mkworld - 生成自定义 planet (world.bin)
 *
 * 基于 ZeroTierOne attic/world/mkworld.cpp 改写（该目录在 1.16 中已被移除）。
 * 与原版不同：根节点从 JSON 读取、支持多个根节点、时间戳默认取当前时间，
 * 这样只要签名密钥不变，已在使用旧 planet 的客户端会通过协议自动更新到新 planet。
 *
 * 用法：
 *   mkworld [-c planet.json] [-o world.bin] [-k 密钥目录]
 *   mkworld -b world.bin            # 把 world.bin 解析为 JSON 输出到 stdout
 *
 * planet.json 格式：
 *   {
 *     "id": 149604618,              // 可选，默认 ZT_WORLD_ID_EARTH
 *     "timestamp": 1700000000000,   // 可选，默认当前时间（毫秒）
 *     "roots": [
 *       { "identity": "xxxxxxxxxx:0:...", "stableEndpoints": ["1.2.3.4/9994"] }
 *     ]
 *   }
 *
 * 密钥目录中需要 previous.c25519 与 current.c25519，不存在时自动生成（两者相同）。
 * 请妥善保管这两个文件：丢失后生成的新 planet 无法被已有客户端自动接受。
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <string>
#include <vector>

#include <nlohmann/json.hpp>

#include <node/Constants.hpp>
#include <node/World.hpp>
// ZeroTier 1.16 起 C25519 更名为 ECC，这里兼容新旧两种源码
#if __has_include(<node/ECC.hpp>)
#include <node/ECC.hpp>
#define ZT_KEY ECC
#define ZT_KEY_PUBLIC_LEN ZT_ECC_PUBLIC_KEY_SET_LEN
#define ZT_KEY_PRIVATE_LEN ZT_ECC_PRIVATE_KEY_SET_LEN
#else
#include <node/C25519.hpp>
#define ZT_KEY C25519
#define ZT_KEY_PUBLIC_LEN ZT_C25519_PUBLIC_KEY_LEN
#define ZT_KEY_PRIVATE_LEN ZT_C25519_PRIVATE_KEY_LEN
#endif
#include <node/Identity.hpp>
#include <node/InetAddress.hpp>
#include <osdep/OSUtils.hpp>

using namespace ZeroTier;
using json = nlohmann::json;

static void printHelp()
{
	printf("Usage:\n");
	printf("  mkworld [-c planet.json] [-o world.bin] [-k keydir]\n");
	printf("  mkworld -b world.bin\n");
}

static bool loadKeyPair(const std::string& path, ZT_KEY::Pair& kp)
{
	std::string s;
	if (! OSUtils::readFile(path.c_str(), s))
		return false;
	if (s.length() != (ZT_KEY_PUBLIC_LEN + ZT_KEY_PRIVATE_LEN))
		return false;
	memcpy(kp.pub.data, s.data(), ZT_KEY_PUBLIC_LEN);
	memcpy(kp.priv.data, s.data() + ZT_KEY_PUBLIC_LEN, ZT_KEY_PRIVATE_LEN);
	return true;
}

static int jsonToBinary(const std::string& configPath, const std::string& outPath, const std::string& keyDir)
{
	const std::string prevPath = keyDir + "/previous.c25519";
	const std::string curPath = keyDir + "/current.c25519";

	if (! OSUtils::fileExists(prevPath.c_str()) && ! OSUtils::fileExists(curPath.c_str())) {
		ZT_KEY::Pair np(ZT_KEY::generate());
		std::string k;
		k.append((const char*)np.pub.data, ZT_KEY_PUBLIC_LEN);
		k.append((const char*)np.priv.data, ZT_KEY_PRIVATE_LEN);
		OSUtils::writeFile(prevPath.c_str(), k);
		OSUtils::writeFile(curPath.c_str(), k);
		fprintf(stderr, "INFO: created initial world keys in %s" ZT_EOL_S, keyDir.c_str());
	}

	ZT_KEY::Pair previousKP, currentKP;
	if (! loadKeyPair(prevPath, previousKP) || ! loadKeyPair(curPath, currentKP)) {
		fprintf(stderr, "FATAL: previous.c25519 or current.c25519 missing or invalid" ZT_EOL_S);
		return 1;
	}

	std::string content;
	if (! OSUtils::readFile(configPath.c_str(), content)) {
		fprintf(stderr, "FATAL: cannot read %s" ZT_EOL_S, configPath.c_str());
		return 1;
	}

	json config;
	try {
		config = json::parse(content);
	}
	catch (std::exception& e) {
		fprintf(stderr, "FATAL: invalid JSON in %s: %s" ZT_EOL_S, configPath.c_str(), e.what());
		return 1;
	}

	const uint64_t id = config.value("id", (uint64_t)ZT_WORLD_ID_EARTH);
	const uint64_t ts = config.value("timestamp", (uint64_t)OSUtils::now());

	std::vector<World::Root> roots;
	if (! config["roots"].is_array() || config["roots"].empty()) {
		fprintf(stderr, "FATAL: roots must be a non-empty array" ZT_EOL_S);
		return 1;
	}
	if (config["roots"].size() > ZT_WORLD_MAX_ROOTS) {
		fprintf(stderr, "FATAL: too many roots (max %d)" ZT_EOL_S, ZT_WORLD_MAX_ROOTS);
		return 1;
	}
	for (auto& r : config["roots"]) {
		World::Root root;
		const std::string ids = r.value("identity", "");
		if (! root.identity.fromString(ids.c_str())) {
			fprintf(stderr, "FATAL: invalid identity: %s" ZT_EOL_S, ids.c_str());
			return 1;
		}
		if (! r["stableEndpoints"].is_array() || r["stableEndpoints"].empty()) {
			char abuf[16];
			fprintf(stderr, "FATAL: root %s has no stableEndpoints" ZT_EOL_S, root.identity.address().toString(abuf));
			return 1;
		}
		for (auto& ep : r["stableEndpoints"]) {
			InetAddress addr(ep.get<std::string>().c_str());
			if (! addr) {
				fprintf(stderr, "FATAL: invalid endpoint: %s" ZT_EOL_S, ep.get<std::string>().c_str());
				return 1;
			}
			if (root.stableEndpoints.size() < ZT_WORLD_MAX_STABLE_ENDPOINTS_PER_ROOT)
				root.stableEndpoints.push_back(addr);
		}
		roots.push_back(root);
	}

	fprintf(stderr, "INFO: generating and signing id==%llu ts==%llu roots==%u" ZT_EOL_S, (unsigned long long)id, (unsigned long long)ts, (unsigned int)roots.size());

	World nw = World::make(World::TYPE_PLANET, id, ts, currentKP.pub, roots, previousKP);

	Buffer<ZT_WORLD_MAX_SERIALIZED_LENGTH> outtmp;
	nw.serialize(outtmp, false);
	World testw;
	testw.deserialize(outtmp, 0);
	if (testw != nw) {
		fprintf(stderr, "FATAL: serialization test failed!" ZT_EOL_S);
		return 1;
	}

	if (! OSUtils::writeFile(outPath.c_str(), std::string((const char*)outtmp.data(), outtmp.size()))) {
		fprintf(stderr, "FATAL: cannot write %s" ZT_EOL_S, outPath.c_str());
		return 1;
	}
	fprintf(stderr, "INFO: %s written with %u bytes" ZT_EOL_S, outPath.c_str(), outtmp.size());
	return 0;
}

static int binaryToJson(const std::string& binPath)
{
	std::string bin;
	if (! OSUtils::readFile(binPath.c_str(), bin)) {
		fprintf(stderr, "FATAL: cannot read %s" ZT_EOL_S, binPath.c_str());
		return 1;
	}
	if (bin.size() > ZT_WORLD_MAX_SERIALIZED_LENGTH) {
		fprintf(stderr, "FATAL: %s is too large" ZT_EOL_S, binPath.c_str());
		return 1;
	}

	World world;
	try {
		Buffer<ZT_WORLD_MAX_SERIALIZED_LENGTH> buf(bin.data(), (unsigned int)bin.size());
		world.deserialize(buf, 0);
	}
	catch (...) {
		fprintf(stderr, "FATAL: cannot parse %s" ZT_EOL_S, binPath.c_str());
		return 1;
	}

	json out;
	out["id"] = world.id();
	out["timestamp"] = world.timestamp();
	out["roots"] = json::array();
	for (const auto& root : world.roots()) {
		char idbuf[ZT_IDENTITY_STRING_BUFFER_LENGTH];
		json r;
		r["identity"] = root.identity.toString(false, idbuf);
		r["stableEndpoints"] = json::array();
		for (const auto& ep : root.stableEndpoints) {
			char ipbuf[64];
			r["stableEndpoints"].push_back(ep.toString(ipbuf));
		}
		out["roots"].push_back(r);
	}
	printf("%s\n", out.dump(2).c_str());
	return 0;
}

int main(int argc, char** argv)
{
	std::string config = "planet.json";
	std::string out = "world.bin";
	std::string keyDir = ".";
	std::string bin;

	for (int i = 1; i < argc; ++i) {
		const std::string a = argv[i];
		const bool hasNext = (i + 1) < argc;
		if (a == "-h" || a == "--help") {
			printHelp();
			return 0;
		}
		else if (a == "-c" && hasNext) {
			config = argv[++i];
		}
		else if (a == "-o" && hasNext) {
			out = argv[++i];
		}
		else if (a == "-k" && hasNext) {
			keyDir = argv[++i];
		}
		else if (a == "-b" && hasNext) {
			bin = argv[++i];
		}
		else {
			printHelp();
			return 1;
		}
	}

	if (! bin.empty())
		return binaryToJson(bin);
	return jsonToBinary(config, out, keyDir);
}
