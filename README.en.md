# StripchatRecorder

[简体中文](README.md) | [English](README.en.md)

A self-hosted Stripchat live stream recorder with a web-based management UI. Supports automatic recording, post-processing pipelines, and multi-channel notifications.

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html)
[![Docker Image](https://img.shields.io/docker/pulls/chantrail/stripchat-recorder)](https://hub.docker.com/r/chantrail/stripchat-recorder)

---

## Features

- Monitor multiple streamers and auto-record when they go live
- Automatic file rotation: set **Recording File Duration (seconds)** in Settings to `1800` for 30-minute files, or `0` (default) for unlimited recording. The value auto-saves and applies to active recordings based on downloaded media duration. Completed files merge in the background while recording continues. Cuts preserve whole HLS chunks, so files can exceed the limit by one chunk; lowering the limit rotates an already-over-limit file when the next chunk arrives.
- Web UI for managing streamers, recordings, and post-processing
- **Streamer Finder**: discover streamers via [camgirlfinder.net](https://camgirlfinder.net), supporting:
  - Face search: upload an image, auto-detect a face, and find similar streamers
  - Name search: search by username keyword
  - One-click add to recording list directly from result cards
  - Launch a similar-face search from any streamer card
- **HLS Relay**: proxy a streamer's live stream to any player without recording — open `/stream/{modelname}` to start automatically; supports multiple simultaneous clients
- Supports split network proxies: configure Stripchat API proxy and CDN chunk proxy separately
- Supports configurable Stripchat mirror site (replaces `stripchat.com` in requests with your mirror domain)
- **Mouflon HLS decryption**: manage `pkey → pdkey` key pairs for decrypting Stripchat's encrypted HLS segment filenames; supports automatic key sync from a configured URL
- **Segments and finished videos kept apart**: `.ts` segments and metadata (`.json`) stay in the output directory while merged videos are written to a separate *Final Video Directory* that holds nothing else
- Configurable post-processing pipeline with pluggable modules:
  - **contact_sheet** — generates a tiled preview image with timestamps
  - **filter_short** — deletes recordings below a minimum duration
  - **notify_discord** — sends recording info and cover image to a Discord Webhook
  - **notify_telegram** — sends recording info, cover image, and video via MTProto (supports files >2 GB, HTTP/SOCKS5 proxy); can create/reuse one group forum topic per model and upload the video into it
- **Manual send to Telegram**: pick any finished video (or several at once) on the recordings page and upload it right away, without running the whole pipeline — group id, per-model topics, and the video toggle can all be overridden per send
- **Delete after upload**: once Telegram accepts the video, the recording's video, `.ts` segments, metadata and side files are removed from the server, so a small VPS doesn't fill up
- Disk space monitoring on the recordings page, with a warning highlight when less than 5 GB remains
- Dual runtime: Tauri desktop app or headless server accessible via browser
- Real-time UI updates via Server-Sent Events with multi-client sync
- Responsive UI: bottom tab bar, card layouts and touch-sized controls on phones, full sidebar layout on desktop
- Dark/light mode following system theme
- Custom UI language support, see [Custom Locale Guide](docs/custom-locale.en.md)

---

## Quick Start (Docker)

### docker-compose (recommended)

```yaml
services:
  stripchat-recorder:
    image: chantrail/stripchat-recorder:latest
    container_name: stripchat-recorder
    restart: unless-stopped
    environment:
      - TZ=Asia/Shanghai
      # - LANGUAGE=en-US  # Set interface language: zh-CN (default) or en-US
      # - PORT=3030        # Set server port (default: 3030)
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

Then open `http://localhost:3030` in your browser.

The Docker image runs in Server mode by default (port 3030). Configuration is written to the mounted `config/settings.json`.

### Key Settings

The following options are available in the Web UI under Settings:

| Setting                        | Description                                                                 |
| ------------------------------ | --------------------------------------------------------------------------- |
| Output directory               | Path where segments and session directories are saved (`.ts` files while recording) |
| Final video directory          | Only merged videos are stored here; `.ts` segments and metadata stay in the output directory. Leave empty to use the output directory |
| Max concurrent                 | Maximum number of simultaneous recordings; `0` means unlimited             |
| Poll interval                  | How often to check if a streamer is live (seconds), range 10–300           |
| Preferred recording resolution | Target stream resolution and fallback direction (prefer lower or prefer higher); if no stream exists in that direction, the nearest stream in the other direction is used instead of failing; defaults to original/highest |
| Merge format                   | Format for auto-merging segments after recording: `mp4` (default), `mkv`, `ts` |
| Auto-record                    | Whether newly added streamers have auto-record enabled by default           |
| Max post-process tmp dir (GB)  | Size limit for temporary files created by post-processing modules; oldest files are deleted when exceeded; `0` means unlimited, default 50 GB |

### Network Proxies and Mirror

In the settings page under "Network", you can configure:

Stripchat mirror project: <https://github.com/ChanTrail/StripchatMirror>

1. API Proxy: used for Stripchat API access; if a mirror is also set, mirror requests go through this proxy.
2. CDN Proxy: used for downloading live stream chunks; can be configured independently from the API proxy.
3. Stripchat Mirror: replaces `stripchat.com` in requests with your mirror domain.

### Cloudflare and Filtered Networks

Stripchat's API is behind Cloudflare, which fingerprints the TLS/HTTP2 handshake. A plain HTTP client is answered with `403` even when it sends a complete set of browser headers, and some ISPs (India, for example) poison the DNS answer for `stripchat.com`, so every request hangs until it times out. Both look like a generic "API error" when adding a streamer.

The backend handles this by itself:

- API requests use a real browser TLS/HTTP2 fingerprint (BoringSSL with a Chrome profile) instead of the default client handshake, which passes Cloudflare's check.
- Hostnames are resolved through DNS-over-HTTPS (Cloudflare `1.1.1.1` and Google), falling back to the system resolver when DoH is unreachable, so a poisoned local resolver no longer breaks the connection.
- When an API proxy is configured, the proxy is used as before and DoH is skipped (the proxy resolves names itself).

If you still get `403` / `451`, the network is blocked at a level the client cannot fix — configure an API Proxy or a mirror.

### Mouflon HLS Decryption Keys

Stripchat encrypts HLS segment filenames (the Mouflon system). If recordings fail to download segments, add the corresponding `pkey → pdkey` key pairs in Settings under "Mouflon Decryption Keys". Keys can be obtained from community channels.

You can also configure a "Sync URL" and optional "Sync Token" to automatically pull the latest keys from a specified URL.

### HLS Relay

In Server mode, you can stream any streamer's live broadcast directly to a player without adding them to the recording list:

```
http://localhost:3030/stream/{modelname}
```

The relay starts automatically on first access and supports multiple simultaneous clients on the same stream. The "Relay" page in the Web UI shows all active sessions with their state, connection count, and uptime.

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

## Post-processing Modules

Modules are standalone executables implementing a simple protocol. They receive input via environment variables and communicate with the host via stdout.

### Built-in Modules

| Module            | Description                                                                    |
| ----------------- | ------------------------------------------------------------------------------ |
| `contact_sheet`   | Extracts frames at a configurable interval and tiles them into a preview image |
| `filter_short`    | Deletes recordings shorter than a configurable minimum duration                |
| `notify_discord`  | Sends recording info and cover image to a Discord Webhook                      |
| `notify_telegram` | Sends recording info, cover image, and video to Telegram via MTProto (supports files >2 GB, HTTP/SOCKS5 proxy); can create/reuse one forum topic per model and post into it |

Custom modules placed in the `modules` volume directory are discovered automatically and will not be overwritten when the container restarts. See the [Module Development Guide](docs/module-development.en.md) for details.

### Testing a Module

On the Post-processing page, modules that support self-testing (declared with `"testable": true` in their metadata, e.g. `notify_telegram`) show a **Test** button on their node. It runs the module in self-test mode (`PP_TEST=1`) with that node's current parameters and streams the output into a dialog:

- The Telegram module verifies the credentials, the target chat and topic resolution/creation, then posts a test message into that chat (or topic). With **send video** enabled it also generates a 3-second test clip and runs the full upload path.
- Nothing is modified on disk; the only side effect is the test message/clip in the configured destination.
- The test uses a fixed topic named `Test`, so repeated tests reuse the same topic instead of creating new ones.

### Telegram Login (user account / large files)

The Telegram module supports three login modes, selected with the `account` parameter (default `auto`):

- `auto`: **a signed-in user session wins**; the bot account is used only when no user session exists. Once you have logged in there is nothing to switch by hand.
- `user`: your own account (used even when a Bot Token is filled in). Modules that support login show a **Login** button on their node; enter your phone number and the login code (plus your password if two-step verification is on) and the session is stored in `~/.config/notify_telegram/<api_id>_user.db`. A user account needs neither `api_hash` nor a Bot Token.
- `bot`: a bot account using a Bot Token. Bots **cannot** list forum topics (`messages.getForumTopics` fails with `BOT_METHOD_INVALID`), so topics rely on the local cache (`~/.config/notify_telegram/topics.tsv`).

A user account can list forum topics, so existing topics are reused reliably and never duplicated. Premium accounts can also upload single files up to 4 GB (2 GB for regular accounts and bots); anything larger is split automatically.

### Telegram forum topics

"Create/reuse a group topic per model" only works in a **supergroup with Topics enabled**, and the account must be an admin with the **Manage Topics** permission. Common setup mistakes:

- **A basic group has no topics.** Turning on Topics in the group settings makes Telegram upgrade the group to a supergroup, and its id changes from `-1234567890` to a `-100…` form — put the new id in Chat ID.
- **Private groups have no username**: leave `username` empty and use the Chat ID only (`-100xxxxxxxxxx` for supergroups). A channel id written as `-<channel_id>`, or without the minus sign, is also understood — the module retries it as a supergroup when the first interpretation does not resolve.
- Topics are optional: turn "Create/reuse a group topic per model" off and messages go straight into the group's main chat (basic groups work fine that way).

### Manual sending and delete-after-upload

Every row on the recordings page has a **Send to Telegram** button (and selecting several files adds one
to the batch bars), so a finished video can be uploaded immediately without waiting for the pipeline:

- The dialog pre-fills the group id, group username, per-model topics and the video toggle from the
  pipeline node. Changes apply to that one send only and are never written back to the pipeline config.
- Upload progress, speed and the module's output are shown live, and the send can be cancelled.
- Multiple files are queued and uploaded one at a time, so the uplink isn't fragmented and Telegram's
  rate limits aren't tripped.

"Delete server data after sending" (the pipeline's `delete_after_send` parameter, on by default) removes
the recording's **video, `.ts` segment directory, metadata file and module side outputs** (contact sheets,
etc.) once Telegram confirms the upload, and drops the entry from the post-processing results list. The
recordings page loses the row immediately and reports how much space was freed. Turn the toggle off to
keep the local files.

---

## Building from Source

**Prerequisites:** Rust (1.98+), Node.js (LTS), ffmpeg, and — for the BoringSSL used by the API client — `cmake`, `perl`, a C/C++ toolchain and `libclang` (for bindgen). On Debian/Ubuntu:

```bash
apt-get install -y build-essential cmake perl libclang-dev
```

`npm run dev`, `npm run check` and `npm run build` find `cargo` (including `~/.cargo/bin`, which is not on `PATH` until you source `~/.cargo/env`) and `libclang` on their own — system installs, LLVM directories and the pip `clang` wheel — and fall back to `pip install libclang` when neither exists. If it lives somewhere unusual, point bindgen at the directory containing the shared library:

```bash
export LIBCLANG_PATH=/path/to/libclang
```

### First-launch Setup

When running the binary directly, if no run mode has been configured in `config/settings.json`, an interactive TUI will guide you through setup:

1. Select UI language (Chinese / English)
2. Select run mode (Desktop / Server)
3. If Server mode, enter the listen port (default 3030)

The choices are saved to `config/settings.json` and the TUI will not appear again on subsequent launches.

```bash
# Install frontend dependencies
npm install

# Build frontend + Tauri binary
npm run build
npx tauri build --no-bundle

# Build post-processing modules
for dir in modules/*/; do
  [ -f "$dir/Cargo.toml" ] && cargo build --manifest-path "$dir/Cargo.toml" --release --bins
done
```

### Build Docker image

```bash
docker build -t chantrail/stripchat-recorder .
```

---

## Tech Stack

- **Frontend:** Vue 3, TypeScript, Vite, Tailwind CSS, Reka UI
- **Backend / Desktop:** Rust, Tauri 2
- **Post-processing modules:** Rust (standalone binaries)
- **Container:** Debian, ffmpeg

---

## License

This project is licensed under the [GNU General Public License v3.0](https://www.gnu.org/licenses/old-licenses/gpl-3.0.html).

---

## Disclaimer

This project is intended for technical research and learning only. Users are responsible for deployment, operations, and compliance risks.
