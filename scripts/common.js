/**
 * 构建脚本公共工具 / Shared build script utilities
 *
 * 供 dev.js / check.js / release.js 引用，避免重复代码。
 * Used by dev.js, check.js, and release.js to avoid duplication.
 */

"use strict";

const { execSync } = require("child_process");
const path = require("path");
const fs   = require("fs");

const { prepareBuildEnv } = require("./env");

// ── 路径常量 / Path constants ────────────────────────────────────────────────

const ROOT        = path.resolve(__dirname, "..");
const FRONTEND    = path.join(ROOT, "frontend");
const DESKTOP     = path.join(ROOT, "desktop");
const MODULES_DIR = path.join(ROOT, "modules");
const BUILD_TMP   = path.join(ROOT, "build_tmp");
const BUILD_OUT   = path.join(ROOT, "build");

/** 后端 Cargo.toml 路径 / Backend Cargo.toml path */
const BACKEND_MANIFEST = path.join(ROOT, "backend", "Cargo.toml");

/** 后端编译产物目录 / Backend target directory */
const BACKEND_TARGET = path.join(BUILD_TMP, "backend", "target");

/** Desktop (Tauri) 编译产物目录 / Desktop (Tauri) target directory */
const DESKTOP_TARGET = path.join(BUILD_TMP, "desktop", "target");

/** 指定模块的编译产物目录 / Target directory for a given module */
function moduleTarget(name) {
  return path.join(BUILD_TMP, "modules", name, "target");
}

/** 枚举所有可构建的模块名（跳过纯库 crate）/ List all buildable module names (skip pure library crates) */
function listModules() {
  const skip = new Set(["pp_utils"]);
  return fs
    .readdirSync(MODULES_DIR)
    .filter(
      (n) => !skip.has(n) && fs.existsSync(path.join(MODULES_DIR, n, "Cargo.toml"))
    );
}

// ── ANSI 颜色 / ANSI colors ──────────────────────────────────────────────────

const C = {
  reset:  "\x1b[0m",
  cyan:   "\x1b[36m",   // header 边框
  yellow: "\x1b[33m",   // step 边框
  gray:   "\x1b[90m",   // 嵌套时的暗色
  bold:   "\x1b[1m",
};

/** 是否作为子进程被嵌套调用（由 dev/release 通过环境变量注入）
 *  Whether running as a nested subprocess (injected by dev/release via env var) */
const NESTED = process.env.CHECK_NESTED === "1";

// ── 输出工具 / Output helpers ────────────────────────────────────────────────

/** 打印脚本开头的总描述标题 / Print the overall script header
 * @param {string} title  标题 / title
 * @param {string} desc   描述 / description
 */
function header(title, desc) {
  if (NESTED) {
    const indent = "    ";
    console.log(`\n${indent}${C.gray}${"┄".repeat(52)}${C.reset}`);
    console.log(`${indent}${C.gray}  ${title}${C.reset}`);
    if (desc) console.log(`${indent}${C.gray}  ${desc}${C.reset}`);
    console.log(`${indent}${C.gray}${"┄".repeat(52)}${C.reset}`);
  } else {
    console.log(`\n${C.cyan}${"═".repeat(60)}${C.reset}`);
    console.log(`${C.bold}  ${title}${C.reset}`);
    if (desc) console.log(`  ${desc}`);
    console.log(`${C.cyan}${"═".repeat(60)}${C.reset}`);
  }
}

/** 打印分隔线步骤标题 / Print a separator with a step title
 * @param {number} current  当前步骤编号 / current step number
 * @param {number} total    总步骤数 / total steps
 * @param {string} msg      步骤描述 / step description
 */
function step(current, total, msg) {
  if (NESTED) {
    const indent = "    ";
    console.log(`\n${indent}${C.gray}[${current}/${total}]  ${msg}${C.reset}`);
  } else {
    console.log(`\n${C.yellow}${"─".repeat(60)}${C.reset}`);
    console.log(`  ${C.bold}[${current}/${total}]${C.reset}  ${msg}`);
    console.log(`${C.yellow}${"─".repeat(60)}${C.reset}`);
  }
}

/**
 * 同步执行命令，继承 stdio / Run a command synchronously, inheriting stdio.
 *
 * 失败时只打印一行可读的错误并以相同退出码退出，而不是抛出整段 Node 栈。
 * On failure prints one readable line and exits with the same code instead of
 * dumping a full Node stack trace. Set DEBUG=1 to keep the stack.
 *
 * @param {string} cmd
 * @param {import("child_process").ExecSyncOptions} [opts]
 */
function run(cmd, opts = {}) {
  try {
    execSync(cmd, { stdio: "inherit", ...opts });
  } catch (err) {
    const code = typeof err.status === "number" && err.status > 0 ? err.status : 1;
    console.error(`\n  ✗ Command failed (exit ${code}): ${cmd}`);
    if (process.env.DEBUG === "1") console.error(err.stack || String(err));
    process.exit(code);
  }
}

// ── 二进制收集 / Binary collection ──────────────────────────────────────────

/**
 * 收集目录顶层的所有可执行文件。
 * Collect all executable files at the top level of a directory.
 * Windows: *.exe；Linux/macOS: 有执行权限且无扩展名的文件。
 * @param {string} releaseDir
 * @returns {string[]}
 */
function collectBinaries(releaseDir) {
  if (!fs.existsSync(releaseDir)) return [];
  const isWindows = process.platform === "win32";
  return fs.readdirSync(releaseDir).filter((name) => {
    const full = path.join(releaseDir, name);
    const stat = fs.statSync(full);
    if (!stat.isFile()) return false;
    if (isWindows) return name.endsWith(".exe");
    return !path.extname(name) && (stat.mode & 0o111) !== 0;
  });
}

/**
 * 同上，但跳过符号链接。
 * Same as above, but symlinks are skipped.
 *
 * cargo 上一次构建遗留的旧版本二进制通常是 `deps/` 里的软链（本环境的 `.l2s.*`），
 * 把它们当成产物复制出去，会让同一个模块 id 在模块目录里出现两份。
 * Leftovers from a previous build are usually symlinks into `deps/` (`.l2s.*`
 * here); copying them out makes one module id show up twice.
 *
 * @param {string} dir
 * @returns {string[]}
 */
function collectRealBinaries(dir) {
  return collectBinaries(dir).filter((name) => {
    try {
      return !fs.lstatSync(path.join(dir, name)).isSymbolicLink();
    } catch {
      return false;
    }
  });
}

/**
 * 通过 `cargo metadata` 取得该包真正产出的二进制名。
 * Ask `cargo metadata` for the binary names the package actually produces.
 *
 * 直接读目录会把上一次构建遗留的旧版本二进制一起算进去，所以这里以包定义为准。
 * Reading the directory would also pick up binaries left over from a previous
 * build, so the package definition is the source of truth.
 *
 * @param {string} manifest Cargo.toml 路径 / path to Cargo.toml
 * @returns {string[]|null} 失败时返回 null / null when it cannot be determined
 */
function packageBinNames(manifest) {
  try {
    const json = execSync(
      `cargo metadata --no-deps --format-version 1 --manifest-path "${manifest}"`,
      { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] }
    );
    const meta = JSON.parse(json);
    const names = [];
    for (const pkg of meta.packages || []) {
      for (const target of pkg.targets || []) {
        if (Array.isArray(target.kind) && target.kind.includes("bin")) {
          names.push(target.name);
        }
      }
    }
    return names.length ? [...new Set(names)] : null;
  } catch {
    return null;
  }
}

/**
 * 解析某模块本次构建实际产出的二进制文件名。
 * Resolve the binary file names a module actually produced in this build.
 *
 * @param {string} name     模块名 / module name
 * @param {string} manifest Cargo.toml 路径 / path to Cargo.toml
 * @param {string} profile  构建模式 / build profile
 * @returns {string[]}
 */
function moduleBinaries(name, manifest, profile) {
  const dir = path.join(moduleTarget(name), profile);
  const names = packageBinNames(manifest) || collectRealBinaries(dir);
  const out = [];
  for (const bin of names) {
    if (fs.existsSync(path.join(dir, bin))) {
      out.push(bin);
    } else if (process.platform === "win32" && fs.existsSync(path.join(dir, `${bin}.exe`))) {
      out.push(`${bin}.exe`);
    }
  }
  return out;
}

// ── 目录列表 / Directory listing ────────────────────────────────────────────

/**
 * 递归打印目录内容（构建完成后展示产物）。
 * Recursively print directory contents (display artifacts after build).
 * @param {string} dir
 * @param {string} [prefix]
 */
function listDir(dir, prefix = "") {
  for (const name of fs.readdirSync(dir).sort()) {
    const full = path.join(dir, name);
    if (fs.statSync(full).isDirectory()) {
      console.log(`  ${prefix}${name}/`);
      listDir(full, prefix + "  ");
    } else {
      const size = (fs.statSync(full).size / 1024).toFixed(0);
      console.log(`  ${prefix}${name}  (${size} KB)`);
    }
  }
}

// ── 模块构建与检查 / Module build & check ───────────────────────────────────

/**
 * 对所有模块执行 cargo check。
 * Run `cargo check` for all modules.
 */
function checkModules() {
  for (const name of listModules()) {
    run(
      `cargo check --manifest-path "${path.join(MODULES_DIR, name, "Cargo.toml")}"`,
      { env: { ...process.env, CARGO_TARGET_DIR: moduleTarget(name) } }
    );
  }
}

/**
 * 删除同一模块遗留的旧版本二进制（如 notify_telegram_v031 → _v040）。
 * 模块 id 来自 --describe，而不是文件名；旧二进制残留会让同一 id 出现两次，
 * 管线节点就可能绑定到旧版本。
 *
 * Remove stale binaries left behind by a previous version of the same module
 * (e.g. notify_telegram_v031 → _v040). A module's id comes from `--describe`,
 * not its filename, so a leftover binary makes the same id show up twice and a
 * pipeline node may bind to the old build.
 *
 * @param {string} outDir  二进制目录 / binary directory
 * @param {string} name    模块名 / module name
 * @param {Set<string>} keep 本次刚复制的文件名 / file names just copied
 */
function pruneStaleModuleBinaries(outDir, name, keep) {
  const prefix = `${name}_v`;
  let entries;
  try {
    entries = fs.readdirSync(outDir);
  } catch {
    return;
  }
  for (const entry of entries) {
    if (!entry.startsWith(prefix) || keep.has(entry)) continue;
    const full = path.join(outDir, entry);
    try {
      const st = fs.statSync(full);
      if (!st.isFile()) continue;
      if (process.platform !== "win32" && (st.mode & 0o111) === 0) continue;
      fs.rmSync(full, { force: true });
      console.log(`  ⚠ removed stale binary ${entry}`);
    } catch {
      /* 删不掉就跳过，不影响构建 / skip if it cannot be removed */
    }
  }
}

/**
 * 构建所有模块并将产物二进制复制到指定目录。
 * Build all modules and copy output binaries to the given directory.
 *
 * @param {"debug"|"release"} profile  Cargo 构建模式 / Cargo build profile
 * @param {string} outDir              二进制复制目标目录 / Target directory for copied binaries
 */
function buildModules(profile, outDir) {
  const releaseFlag = profile === "release" ? " --release" : "";
  fs.mkdirSync(outDir, { recursive: true });
  for (const name of listModules()) {
    console.log(`  → ${name}`);
    const manifest = path.join(MODULES_DIR, name, "Cargo.toml");
    run(`cargo build --manifest-path "${manifest}" --bins${releaseFlag}`, {
      env: { ...process.env, CARGO_TARGET_DIR: moduleTarget(name) },
    });
    const bins = moduleBinaries(name, manifest, profile);
    for (const bin of bins) {
      const dst = path.join(outDir, bin);
      fs.copyFileSync(path.join(moduleTarget(name), profile, bin), dst);
      if (process.platform !== "win32") fs.chmodSync(dst, 0o755);
    }
    pruneStaleModuleBinaries(outDir, name, new Set(bins));
    console.log(`  ✓ ${name}\n`);
  }
}

/**
 * 递归复制目录（构建完成后收集产物用）。
 * Recursively copy a directory (used for collecting build artifacts).
 *
 * @param {string} src       源目录 / Source directory
 * @param {string} dst       目标目录 / Destination directory
 * @param {string} [logBase] 用于日志输出的基准路径前缀 / Base path prefix for log output
 */
function copyDir(src, dst, logBase) {
  fs.mkdirSync(dst, { recursive: true });
  for (const entry of fs.readdirSync(src)) {
    const srcPath = path.join(src, entry);
    const dstPath = path.join(dst, entry);
    if (fs.statSync(srcPath).isDirectory()) {
      copyDir(srcPath, dstPath, logBase);
    } else {
      fs.copyFileSync(srcPath, dstPath);
      if (logBase !== undefined) {
        console.log(`  ✓ ${logBase}${path.relative(dst, dstPath).replace(/\\/g, "/")}`);
      }
    }
  }
}

// ── 前端依赖安装 / Frontend dependency install ──────────────────────────────

/**
 * 安装前端 npm 依赖（每次执行脚本前调用）。
 * Install frontend npm dependencies (called before each script runs).
 */
function installFrontend() {
  console.log("Installing frontend dependencies...");
  run("npm install", { cwd: FRONTEND });
}

/**
 * 安装 desktop npm 依赖（每次执行 desktop 脚本前调用）。
 * Install desktop npm dependencies (called before each desktop script runs).
 */
function installDesktop() {
  console.log("Installing desktop dependencies...");
  run("npm install", { cwd: DESKTOP });
}

// ── 导出 / Exports ───────────────────────────────────────────────────────────

// 所有脚本在 require 本模块时即完成环境准备：cargo 在 PATH 上、
// LIBCLANG_PATH 指向 libclang（bindgen 编译 btls-sys 时需要）。
// Every script gets a prepared environment the moment it requires this module:
// cargo on PATH and LIBCLANG_PATH pointing at libclang (bindgen needs it to
// build btls-sys).
prepareBuildEnv({ quiet: NESTED });

module.exports = {
  ROOT,
  FRONTEND,
  DESKTOP,
  MODULES_DIR,
  BUILD_TMP,
  BUILD_OUT,
  BACKEND_MANIFEST,
  BACKEND_TARGET,
  DESKTOP_TARGET,
  NESTED,
  moduleTarget,
  listModules,
  step,
  header,
  run,
  collectBinaries,
  listDir,
  checkModules,
  buildModules,
  copyDir,
  installFrontend,
  installDesktop,
  prepareBuildEnv,
};
