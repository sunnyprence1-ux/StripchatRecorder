# StripchatRecorder

[简体中文](README.md) | [English](README.en.md)

自托管的 Stripchat 直播录制工具，提供基于 Web 的管理界面，支持自动录制、后处理流水线和多渠道通知。

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html)
[![Docker Image](https://img.shields.io/docker/pulls/chantrail/stripchat-recorder)](https://hub.docker.com/r/chantrail/stripchat-recorder)

---

## 功能特性

- 监控多个主播，上线时自动开始录制
- Web UI 管理主播、录制文件和后处理任务
- **主播查找**：通过 [camgirlfinder.net](https://camgirlfinder.net) 查找主播，支持：
  - 人脸识别搜索：上传图片自动检测人脸，找出相似主播
  - 名字搜索：按用户名关键词搜索
  - 从结果卡片直接一键添加到录制列表
  - 对任意主播发起相似人脸搜索
- **转发流（HLS Relay）**：无需录制即可将主播直播流转发给播放器，访问 `/stream/{modelname}` 自动启动，支持多客户端同时连接
- 支持分离式网络代理：可分别配置 Stripchat API 代理与 CDN 分片代理
- 支持配置 Stripchat 镜像站（将请求中的 `stripchat.com` 替换为镜像域名）
- **Mouflon HLS 解密**：支持管理 `pkey → pdkey` 密钥对，用于解密 Stripchat 加密的 HLS 分片文件名；支持配置同步 URL 自动拉取最新密钥
- **分片与成片分离存放**：`.ts` 分片与元数据（`.json`）保留在输出目录，合并完成的视频单独保存到「最终视频目录」，该目录只含成片
- 可配置的后处理流水线，支持插件化模块：
  - **contact_sheet** — 生成带时间戳的缩略图预览图
  - **filter_short** — 删除低于最短时长的录制文件
  - **notify_discord** — 通过 Discord Webhook 发送录制信息和封面图
  - **notify_telegram** — 通过 MTProto 发送视频（支持超过 2 GB 的大文件，支持 HTTP/SOCKS5 代理）；默认只发视频、不带任何文字，文字说明和封面图可单独开关；可按模特自动创建/复用群组话题（Topic）并把视频发到对应话题
- **手动发送到 Telegram**：在录制文件页挑选任意成片（可多选）立即上传，无需运行整条流水线；群组 ID、按模特分话题、是否发送视频文件、是否附带文字说明、是否发送封面图都可按次覆盖
- **上传后删除**：Telegram 接收成功后，自动删除该录制的成片、`.ts` 分片、元数据和副产物，小内存 VPS 不会被录满
- 录制文件页磁盘空间监控，剩余空间不足 5 GB 时高亮提示
- 双运行模式：可作为 Tauri 桌面应用或无头服务器通过浏览器访问
- 基于 SSE 的实时 UI 更新，支持多客户端同步
- 移动端自适应界面：手机上是底部标签栏 + 卡片式列表 + 触摸尺寸的按钮，桌面端保持侧边栏布局
- 跟随系统主题的深色/浅色模式
- 支持自定义界面语言，详见[自定义语言文档](docs/custom-locale.md)

---

## 快速开始（Docker）

### docker-compose（推荐）

```yaml
services:
  stripchat-recorder:
    image: chantrail/stripchat-recorder:latest
    container_name: stripchat-recorder
    restart: unless-stopped
    environment:
      - TZ=Asia/Shanghai
      # - LANGUAGE=en-US  # 设置界面语言，支持 zh-CN（默认）或 en-US
      # - PORT=3030        # 设置服务端口（默认 3030）
    ports:
      - "${PORT:-3030}:${PORT:-3030}"
    volumes:
      - ./data/logs:/app/stripchat-recorder/logs
      - ./data/recordings:/app/stripchat-recorder/recordings
      - ./data/modules:/app/stripchat-recorder/modules
      - ./data/config:/app/stripchat-recorder/config
```

```bash
docker compose up -d
```

启动后在浏览器中打开 `http://localhost:3030`。

Docker 镜像默认以 Server 模式运行（端口 3030），配置写入挂载的 `config/settings.json`。

### 主要设置项

在 Web UI 的「设置」页面可配置以下选项：

| 设置项                   | 说明                                                                 |
| ------------------------ | -------------------------------------------------------------------- |
| 输出目录                 | 分片与会话目录的保存路径（录制过程中的 `.ts` 文件）                  |
| 最终视频目录             | 仅保存合并完成的视频；`.ts` 分片与元数据仍留在输出目录，留空则与输出目录相同 |
| 最大并发录制数           | 同时录制的最大主播数，`0` 表示不限制                                 |
| 轮询间隔                 | 检查主播是否上线的间隔（秒），范围 10–300                            |
| 首选录制分辨率         | 设置目标分辨率和回退方向（优先较低或较高）；若该方向没有可用流，则使用反方向最接近的画质而不会失败；默认为原始/最高画质 |
| 合并格式                 | 录制结束后自动合并分片的格式：`mp4`（默认）、`mkv`、`ts`            |
| 上线自动录制             | 新添加的主播是否默认开启自动录制                                     |
| 后处理临时目录最大占用   | 后处理模块运行时产生的临时文件上限（GB），超出后自动删除最旧的文件，`0` 表示不限制，默认 50 GB |

### 网络代理与镜像站

在设置页的「网络」中可分别配置：

Stripchat 镜像站项目：<https://github.com/ChanTrail/StripchatMirror>

1. API 代理：用于访问 Stripchat API；若同时填写镜像站，则通过该代理访问镜像站。
2. CDN 代理：用于下载直播分片流，可与 API 代理分开设置。
3. Stripchat 镜像站：用于替换请求中的 `stripchat.com` 域名。

### Cloudflare 与网络受限环境

Stripchat 的 API 位于 Cloudflare 之后，Cloudflare 会对 TLS / HTTP2 握手做指纹校验：普通 HTTP 客户端即使带上完整的浏览器请求头也会被返回 `403`；同时部分运营商（例如印度）会污染 `stripchat.com` 的 DNS 解析结果，导致请求一直卡到超时。这两种情况在添加主播时都表现为一个笼统的「API 错误」。

后端已内置处理：

- API 请求使用真实浏览器的 TLS / HTTP2 指纹（BoringSSL + Chrome 配置），可通过 Cloudflare 校验。
- 域名通过 DNS-over-HTTPS（Cloudflare `1.1.1.1` 与 Google）解析，DoH 不可用时自动回退到系统解析，被污染的本地 DNS 不再导致连接失败。
- 配置了 API 代理时按原逻辑走代理，并跳过 DoH（由代理解析域名）。

若仍然返回 `403` / `451`，说明网络在客户端无法绕过的层面被封锁，请配置 API 代理或镜像站。

### Mouflon HLS 解密密钥

Stripchat 对 HLS 分片文件名进行了加密（Mouflon 系统）。若录制时遇到无法下载分片的情况，需在设置页的「Mouflon 解密密钥」中填入对应的 `pkey → pdkey` 密钥对。密钥可从社区渠道获取。

也可以配置「同步地址」和「同步令牌」，从指定 URL 自动拉取最新密钥。

### 转发流（HLS Relay）

在 Server 模式下，无需将主播添加到录制列表，直接用播放器打开以下地址即可播放直播：

```
http://localhost:3030/stream/{modelname}
```

首次访问时自动连接上游，支持多个客户端同时连接同一转发流。在 Web UI 的「转发流」页面可查看所有活跃会话的状态、连接数和运行时长。

### docker run

```bash
docker run -d \
  --name stripchat-recorder \
  --restart unless-stopped \
  -e TZ=Asia/Shanghai \
  -e LANGUAGE=en-US \
  -e PORT=3030 \
  -p 3030:3030 \
  -v ./data/logs:/app/stripchat-recorder/logs \
  -v ./data/recordings:/app/stripchat-recorder/recordings \
  -v ./data/modules:/app/stripchat-recorder/modules \
  -v ./data/config:/app/stripchat-recorder/config \
  chantrail/stripchat-recorder:latest
```

---

## 后处理模块

模块是实现了简单协议的独立可执行文件，通过环境变量接收输入，通过标准输出与主程序通信。

### 内置模块

| 模块              | 说明                                                |
| ----------------- | --------------------------------------------------- |
| `contact_sheet`   | 按配置间隔截帧并拼合为预览图                        |
| `filter_short`    | 删除低于最短时长的录制文件                          |
| `notify_discord`  | 通过 Discord Webhook 发送录制信息和封面图           |
| `notify_telegram` | 通过 MTProto 向 Telegram 发送录制信息、封面图和视频，可按模特创建/复用群组话题（Topic）并把视频发到对应话题 |

将自定义模块放入 `modules` 数据卷目录后会被自动发现，且不会在容器重启时被覆盖。详见[后处理模块开发文档](docs/module-development.md)。

### 模块测试

在「后处理」页面，支持自检的模块（元数据中声明 `"testable": true`，例如 `notify_telegram`）节点上会出现**测试**按钮。
点击后会使用该节点当前的参数、以 `PP_TEST=1` 自检模式运行模块，并在对话框中实时显示输出日志：

- Telegram 模块会依次验证凭据、目标群组、话题解析/创建，然后向该群组（或话题）发送一条测试消息；
  若开启了「同时发送视频文件」，还会现场生成一个 3 秒的测试视频并完整走一遍上传流程。
- 自检不会修改任何录像文件，仅在配置的目标中留下一条测试消息/视频。
- 测试使用固定话题名 `Test`，重复测试会复用同一话题，不会重复创建。

### Telegram 登录（用户账号 / 大文件）

Telegram 模块支持三种登录方式，由参数 `account` 选择（默认 `auto`）：

- `auto`：**已登录的用户会话优先**，没有用户会话时才使用机器人账号。登录过一次之后不必再手动切换。
- `user`：使用你自己的账号（即使同时填了 Bot Token 也只用账号）。支持登录的模块会在节点上显示**登录**按钮，
  按提示输入手机号、验证码（如开启两步验证还需输入密码）即可，登录结果保存在
  `~/.config/notify_telegram/<api_id>_user.db`。用户账号不需要 `api_hash` 与 Bot Token。
- `bot`：使用 Bot Token 的机器人账号。机器人**无法**列出群组话题（`messages.getForumTopics`
  会返回 `BOT_METHOD_INVALID`），因此话题只能依赖本机缓存（`~/.config/notify_telegram/topics.tsv`）。

用户账号可以列出群组话题，因此能可靠复用同名话题、不会重复创建。另外，高级会员（Premium）
的单文件上传上限为 4 GB（普通账号与机器人为 2 GB），超过上限的文件会自动分割。

### Telegram 话题（Topic）要求

「为每个模特创建/复用群组话题」只对**已开启话题的超级群组**有效，并且账号需要是群管理员并具备
「管理话题」（Manage Topics）权限。常见配置问题：

- **普通群（未升级为超级群组）没有话题**。在群设置里打开「话题」后，Telegram 会把群升级为超级群组，
  群 ID 也会从 `-1234567890` 变成 `-100…` 的形式；请把新的 ID 填到 Chat ID，并关闭/重开话题开关以刷新缓存。
- **私有群组没有 username**：`username` 参数留空，只填 Chat ID（超级群组形如 `-100xxxxxxxxxx`）。
  写成 `-<channel_id>` 或省略负号的 `100…` 也能识别，模块会在解析失败时自动按超级群组再试一次。
- 不想用话题也可以：把「为每个模特创建/复用群组话题」关掉，消息会直接发到群的主聊天里（普通群同样可用）。

### 只发视频（不带文字）

「附带文字说明」（参数 `send_caption`）**默认关闭**：上传的视频消息里没有任何文字，
不会再有 `ModelName / Timestamp / Duration / FileName / FileSize` 这段说明。
需要时在流水线节点或发送对话框里打开即可。

「同时发送封面图」（参数 `send_cover`，默认开启）控制相册里是否包含联系表截图；
把它也关掉，群里就只会出现一条纯视频消息。

> 视频、封面图、文字说明三项不能同时关闭——全关时模块会直接报错
> `nothing to send`，而不是连上 Telegram 后才发现没东西可发。

### 手动发送与上传后删除

录制文件页的每一行都有 **发送到 Telegram** 按钮（勾选多个文件后，顶部/底部的操作栏里也会出现），
可以直接把成片发送到群组，不必等后处理流水线：

- 对话框里的群组 ID、群组 Username、按模特分话题、是否发送视频文件、是否附带文字说明、
  是否发送封面图都会预填为流水线节点的当前值，改动只影响这一次发送，不会写回流水线配置。
- 发送过程中实时显示上传进度、速度与模块输出；可以随时取消。
- 多个文件会排队串行上传，避免把上行带宽切碎、也避免触发 Telegram 限流。

「发送成功后删除服务器数据」（流水线参数 `delete_after_send`，默认开启）在 Telegram 确认接收后，
删除该录制的**成片、`.ts` 分片目录、元数据文件和模块副产物**（如联系表图片），
并把该记录从后处理结果列表中移除；录制文件页会立刻少一行，并提示释放了多少空间。
如果只想保留本地文件，把该开关关掉即可。

### 上传速度

上传改成 **512KB 分块、8 块并发**，而不是逐块串行。加速完全来自这里：旧逻辑每发一块都要等服务器
回包，一个 100MB 的视频就是约 200 次串行往返，每一次都要付完整的往返时延。

`upload_workers`（默认 **8**，上限 16）控制同时上传的分块数。每个分块仍然独立重试，所以断线后是
续传而不是整份重来。

在拥堵的手机上行链路上、走真实发送路径、同一个 12MB 视频、交替运行实测：

| 轮次 | 逐块串行 | 8 块并发 | 加速 |
| --- | --- | --- | --- |
| 1 | 58.3s | 27.3s | 2.1 倍 |
| 2 | 112.7s | 76.7s | 1.5 倍 |
| 3 | 48.3s | 32.1s | 1.5 倍 |

中位数 **1.8 倍**（约 205 → 375 KB/s）。并发在每一轮都赢，但收益受链路限制：350 KB/s 的链路上，
串行本来就已经在离上限两倍以内，能赢的空间有限。反过来，**往返时延高、带宽还有余量**的链路
（远距离 VPS、代理、卫星链路）收益会大得多，因为那才是串行往返主导的场景。

> **真正的上限是上行带宽。** 任何上传器都不可能超过你的带宽；并发只是让你别停在远低于带宽的地方。

开发过程中也实现并实测过 Telethon + FastTelethon 侧车，最终**移除**了——原因见下。

#### 为什么没用 Telethon + FastTelethon

做了、测了、然后去掉了，两个原因：

1. **更慢。** 同样的文件和链路，内置上传器在每一次成对对比里都更快：12MB 时 167 vs 132 KB/s，
   40MB 时 438 vs 310 KB/s。侧车要付出进程启动、会话解析和 Python 层的开销，多条连接的握手成本
   只有文件非常大时才摊得薄。
2. **它会破坏会话。** Telethon 必须复用你的登录，也就是共用同一个 auth key。当 grammers 的连接还活着
   时，Telegram 会开始按 Telethon（更高）的 layer 序列化响应，grammers 解析不了更新的
   `message#7600b9d3` 构造器——之后每次发送都失败并报 `unexpected constructor: 7600b9d3`。
   先上传、之后再连 grammers 是可行的，但为了一个更慢的路径把整个发送流程改成那样并不值得。

让上传变快的是并发，而内置上传器已经做到了。

---
---

## 从源码构建

**前置依赖：** Rust（1.98+）、Node.js (LTS)、ffmpeg；另外 API 客户端使用的 BoringSSL 需要 `cmake`、`perl`、C/C++ 工具链以及 `libclang`（bindgen 需要）。Debian/Ubuntu 下：

```bash
apt-get install -y build-essential cmake perl libclang-dev
```

`npm run dev`、`npm run check`、`npm run build` 会自动找到 `cargo`（包括尚未 `source ~/.cargo/env` 时的 `~/.cargo/bin`）和 `libclang`（系统安装、LLVM 目录、pip 的 `clang` wheel 都认），两者都找不到时还会自动 `pip install libclang`，无需手动配置环境变量。只有装在特殊位置时才需要指定（目录内含 `libclang.so`）：

```bash
export LIBCLANG_PATH=/path/to/libclang
```

### 首次启动配置

直接运行二进制文件时，若 `config/settings.json` 中尚未配置运行模式，会自动进入命令行 TUI 引导配置：

1. 选择界面语言（中文 / English）
2. 选择运行模式（Desktop 桌面端 / Server 服务器端）
3. Server 模式下输入监听端口（默认 3030）

配置完成后写入 `config/settings.json`，下次启动直接读取，不再弹出配置界面。

```bash
# 安装前端依赖
npm install

# 构建前端 + Tauri 二进制
npm run build
npx tauri build --no-bundle

# 构建后处理模块
for dir in modules/*/; do
  [ -f "$dir/Cargo.toml" ] && cargo build --manifest-path "$dir/Cargo.toml" --release --bins
done
```

### 构建 Docker 镜像

```bash
docker build -t chantrail/stripchat-recorder .
```

---

## 技术栈

- **前端：** Vue 3, TypeScript, Vite, Tailwind CSS, Reka UI
- **后端 / 桌面端：** Rust, Tauri 2
- **后处理模块：** Rust（独立二进制）
- **容器：** Debian, ffmpeg

---

## 开源许可证

本项目基于 [GNU 通用公共许可证 v3.0](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html) 发布。

---

## 免责声明

本项目仅用于技术研究与学习交流。使用者需自行承担部署、运维与合规风险。
