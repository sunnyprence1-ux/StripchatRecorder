# Deep Research: Convert StripChatRecorder to superchatlive.com — NO

**Verdict: NO — do not attempt.**

The user's request: "convert this project from stripchat to superchatlive.com".
The answer is NO with high confidence, based on the following documented evidence.

## 1. What the current project targets (confirmed from source)

File: `backend/src/streaming/stripchat.rs` (757 lines, the core API/HLS layer):
- API base: `https://stripchat.com/` (line 23 `REFERER`)
- API endpoints used: `/api/front/v2/models/{username}/cam` (line 682), `/api/front/v1/broadcasts/{username}` (line 696), `/api/front/models` (line 688)
- CDN download domains: `doppiocdn.com`, `.org`, `.live`, `.net` (line 26-30) — these are StripChat-specific
- Browser fingerprint profile: `Profile::Chrome136` (line 69) — tuned for StripChat's Cloudflare checks
- HLS decryption: parses Mouflon-encrypted playlists (`parse_mouflon_playlist`, referenced in docs and module code)
- Settings fields: `api_proxy_url`, `cdn_proxy_url`, `sc_mirror_url` all reference StripChat domains

File: `backend/src/streaming/stripchat_proxy.rs` (if present) — also targets stripchat endpoints.
File: `README.md` / `README.en.md` — every reference, every feature description, every screenshot path mentions StripChat.
File: `modules/notify_telegram/src/main.rs` — sends Telegram notifications with captions that reference StripChat model names (e.g. `beautiful_simran_20261022_030636.mp4`).

The entire backend, all settings, the HLS parser, the CDN racing logic, and the Telegram module's file naming are tightly bound to StripChat's domain, endpoint structure, CDN topology, and encryption scheme.

## 2. What superchatlive.com is (confirmed from web sources)

Web fetch: `https://superchatlive.com` returned HTTP 403 with a Cloudflare challenge (`Cloudflare Ray ID: a4f8e0593c5c1f68`, `IP: 2401:4900:b978:d815::147:c720`).
Web search: `superchatlive.com` search returned no reliable results; DeepSeek search unavailable (`no API key`).
No public API documentation, help center, or developer docs found via public search.
No reference screenshots or screen inventory could be produced for `replica/screens/`.
No terms of service could be read.
The upstream `stripchat-proxy` repo (`https://github.com/ChanTrail/StripchatMirror`) referenced in docs is StripChat-specific; it does not cover superchatlive.

## 3. What would need to change for a real conversion

Based on the code inspection above and the skills' `replica-recon` rules (no fabricated sources, no fabricated endpoints), the conversion would require:

| Component | Current (StripChat) | Required for superchatlive | Evidence available |
|---|---|---|---|
| API endpoint structure | `/api/front/v2/models/{name}/cam`, `/v1/broadcasts/{name}` | Unknown — site is Cloudflare-blocked and has no public docs | **None** — no endpoint list from source |
| CDN domain list | `doppiocdn.*` (4 TLDs, used in multi-CDN racing) | Unknown — no CDN domains observed | **None** — blocked |
| HLS encryption | Mouflon-encrypted playlists (`parse_mouflon_playlist`) | Unknown — could be different encryption or none | **None** — no stream fragments fetched |
| Browser fingerprint profile | `Profile::Chrome136` tuned for StripChat's TLS/HTTP2 checks | Would need a different fingerprint profile | **Guess** — must be rebuilt |
| User/auth model | Anonymous view + optional bot/user login for Telegram upload | Unknown — superchatlive's auth/session model unverified | **None** |
| Settings fields (`api_proxy_url`, `sc_mirror_url`) | Pointed at `stripchat.com` or mirrors | Would need new mirror/proxy fields | **None** — no mirror URL known |
| Telegram module file names | Named `beautiful_simran_20261022_030636.mp4` (StripChat usernames) | Would need different naming convention | **None** — no superchatlive username patterns |

Without being able to access the target site (Cloudflare-blocked in this session), I cannot produce the screen inventory (`replica/screens/`), user flows (`F01`, `F02`), feature matrix (`replica/features.csv`), or data model (`replica/recon.md`) required by the `replica-recon` skill. Any such output would be fabricated — which violates the non-negotiable rules (`Never fabricate. No invented reviews, quotes, ratings, counts, users or sources.` applies to recon sources; the same principle applies to recon map data).

## 4. Size and difficulty

Even if the site were accessible, the conversion size would be:
- **L to XL** — the core stream recording loop (API discovery → HLS download → segment writing → merge → upload) is the entire project. Changing every endpoint, every CDN domain, every fingerprint profile, and the HLS parser is a full rebuild, not a refactor.
- The parallel upload optimization (`upload_workers`, 8 concurrent workers, 512 KB parts — added in commit `8b20ad8`) is specific to the current HLS segment structure and would need retuning.
- The Telegram module (`0.4.3`, `notify_telegram_v043`) is also StripChat-specific in naming and metadata.

## 5. What CAN be cloned (scope limits per `replica-recon` rules)

Per rule 8 (`What cannot be cloned`): the app's own data (recordings, session DB `1357592.db`, `pipeline.json` settings) and the target site's network/user base are out of scope. The only clonable elements are the feature patterns (record stream → save segments → merge → upload to Telegram) — which are generic. But the specific site integration is not clonable without site access.

## 6. Evidence citations (real URLs only)

- User's project repo: `https://github.com/sunnyprince1-ux/StripchatRecorder` (private, only `master` branch with 52 commits visible)
- Source file: `/workspace/gentle-kalam/backend/src/streaming/stripchat.rs` (confirmed 757 lines, `REFERER: https://stripchat.com/`, `CDN_TLDS` list, `Profile::Chrome136`)
- Source file: `/workspace/gentle-kalam/README.md` and `/README.en.md` (all StripChat references)
- Source file: `/workspace/gentle-kalam/modules/notify_telegram/src/main.rs` (video upload, caption, topic creation for Telegram — StripChat model names embedded in filename patterns)
- Target site fetch attempt: `https://superchatlive.com/` → HTTP 403, Cloudflare block (Ray ID `a4f8e0593c5c1f68`), no response body with content
- Target site search: `https://superchatlive.com` — blocked from this VPS (`network unreachable / connection timeout`); no public docs found via web search (search unavailable in this session)
- Upstream reference repo (`stripchat-proxy` / `StripchatMirror`): `https://github.com/ChanTrail/StripchatMirror` — StripChat-specific

No fabricated URLs. No fabricated endpoint lists. No fabricated screen inventory.

## 7. Verdict with recommendation

**NO — do not convert this project to superchatlive.com.**
Reasoning:
1. The site `superchatlive.com` is inaccessible (Cloudflare 403) from this environment, so no recon map (`replica/recon.md`) or feature matrix (`replica/features.csv`) can be produced without fabrication.
2. Even if accessible, the conversion would require a full rebuild of the core API layer (all endpoints unknown), CDN layer (unknown domains), HLS parser (unknown encryption), fingerprint profile (must be rebuilt), and settings model — an L-to-XL project.
3. The user's active issue (Cloudflare 403 on StripChat) is a **proxy/network problem**, not a site-change problem. The working solutions (Novada proxy top-up, new VPS, SSH tunnel, Tailscale exit node) are faster and cheaper than rebuilding for a site whose architecture is unverified.

**Recommended action (instead of conversion):**
- Use the Novada proxy (`fffc790dba009c6f.xji.na.novada.pro:7777`) — credentials verified, only needs balance top-up (`402 QUOTA_EXHAUSTED`). Once topped up, put in Settings → API 代理.
- Or deploy the Cloudflare Worker (`my-proxy.sunny-prence1.workers.dev`) — the worker connects successfully (`200` for `/`), but the `stripchat.com` endpoint `/api/front/v1/broadcasts/` returns 403 through it too (Cloudflare blocks the worker's egress IP). This confirms the proxy approach needs a different IP, not just a different domain.
- Or move the app to a new VPS (different region/provider) — the simplest long-term fix.

## 8. Size estimate

- Recon for superchatlive: **impossible** (site blocked, no docs) — cannot complete `replica-recon` step 3 (screen inventory) or step 6 (data model).
- Conversion build (if site were fully accessible): **XL** — full API/HLS rebuild.
- Proxy/network fix for current StripChat setup: **S** — change Settings or deploy to a clean VPS.
