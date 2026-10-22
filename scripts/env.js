/**
 * 构建环境准备 / Build environment preparation
 *
 * 后端依赖 wreq（BoringSSL TLS 指纹），其 btls-sys 构建脚本在编译期用 bindgen
 * 生成绑定，因此需要 libclang 动态库；此外所有步骤都要求 cargo 在 PATH 上。
 * 本模块自动定位（必要时安装）这两者，并把结果写入 process.env，
 * 使 dev / check / release 派生出的所有子进程都能继承。
 *
 * The backend depends on wreq (BoringSSL TLS fingerprinting); its btls-sys build
 * script runs bindgen at compile time, which needs the libclang shared library.
 * Every step also needs cargo on PATH. This module locates (and if necessary
 * installs) both, then exports them through process.env so that every child
 * process spawned by dev / check / release inherits them.
 */

"use strict";

const { execFileSync } = require("child_process");
const path = require("path");
const fs   = require("fs");
const os   = require("os");

const isWindows = process.platform === "win32";
const isMac     = process.platform === "darwin";

// ── 小工具 / Small helpers ───────────────────────────────────────────────────

/** 该文件是否存在且可执行 / Whether the file exists and is executable */
function isExecutable(file) {
  try {
    const st = fs.statSync(file);
    if (!st.isFile()) return false;
    if (isWindows) return true; // cargo.exe 的存在即可 / existence is enough
    return (st.mode & 0o111) !== 0;
  } catch {
    return false;
  }
}

/** 展开路径中的 * 通配（仅返回目录）/ Expand `*` segments in a path (directories only) */
function globDirs(pattern) {
  if (!pattern.includes("*")) {
    try {
      return fs.statSync(pattern).isDirectory() ? [pattern] : [];
    } catch {
      return [];
    }
  }

  const isAbs = path.isAbsolute(pattern);
  const segments = pattern.split(path.sep).filter((s) => s !== "");
  let current = [isAbs ? path.sep : "."];

  for (const seg of segments) {
    const next = [];
    for (const base of current) {
      if (!seg.includes("*")) {
        const full = path.join(base, seg);
        try {
          if (fs.statSync(full).isDirectory()) next.push(full);
        } catch {
          /* 不存在 / missing */
        }
        continue;
      }
      const prefix = seg.slice(0, seg.indexOf("*"));
      const suffix = seg.slice(seg.lastIndexOf("*") + 1);
      let entries = [];
      try {
        entries = fs.readdirSync(base);
      } catch {
        continue;
      }
      for (const name of entries) {
        if (!name.startsWith(prefix) || !name.endsWith(suffix)) continue;
        const full = path.join(base, name);
        try {
          if (fs.statSync(full).isDirectory()) next.push(full);
        } catch {
          /* 悬空链接等 / dangling link */
        }
      }
    }
    current = next;
  }
  return current;
}

// ── cargo ────────────────────────────────────────────────────────────────────

/** cargo 可能所在的目录 / Directories where cargo may live */
function cargoDirCandidates() {
  const home = os.homedir();
  const dirs = [];

  if (process.env.CARGO_HOME) dirs.push(path.join(process.env.CARGO_HOME, "bin"));
  dirs.push(path.join(home, ".cargo", "bin"));
  // Termux / proot 里 rustup 装到 $PREFIX 或 /usr/local/cargo 的情况
  // rustup under Termux / proot may install to $PREFIX or /usr/local/cargo
  if (process.env.PREFIX) dirs.push(path.join(process.env.PREFIX, "bin"));
  dirs.push("/usr/local/cargo/bin", "/opt/rust/bin", "/usr/local/bin", "/usr/bin");

  return [...new Set(dirs)];
}

/**
 * 确保 cargo 可用，必要时把它的目录加入 PATH。
 * Ensure cargo is reachable, adding its directory to PATH when needed.
 *
 * @returns {boolean} 是否可用 / whether cargo is available
 */
function ensureCargo() {
  if (which("cargo")) return true;

  for (const dir of cargoDirCandidates()) {
    if (isExecutable(path.join(dir, isWindows ? "cargo.exe" : "cargo"))) {
      process.env.PATH = `${dir}${path.delimiter}${process.env.PATH || ""}`;
      return true;
    }
  }
  return false;
}

/** 在 PATH 上查找可执行文件 / Look up an executable on PATH */
function which(cmd) {
  const exts = isWindows
    ? (process.env.PATHEXT || ".EXE;.CMD;.BAT").split(";").filter(Boolean)
    : [""];
  for (const dir of (process.env.PATH || "").split(path.delimiter)) {
    if (!dir) continue;
    for (const ext of exts) {
      const full = path.join(dir, cmd + ext.toLowerCase());
      if (isExecutable(full)) return full;
      const fullUpper = path.join(dir, cmd + ext);
      if (ext !== ext.toLowerCase() && isExecutable(fullUpper)) return fullUpper;
    }
  }
  return null;
}

// ── libclang ─────────────────────────────────────────────────────────────────

/** bindgen 认可的 libclang 文件名 / libclang file names bindgen accepts */
const LIBCLANG_RE = /^libclang(-[^.]*)?\.so(\..*)?$|^libclang\.(dylib|dll)$/;

/** 该目录下是否存在 libclang 动态库 / Does the directory contain a libclang shared library */
function hasLibclang(dir) {
  if (!dir) return false;
  try {
    return fs.readdirSync(dir).some((n) => LIBCLANG_RE.test(n));
  } catch {
    return false;
  }
}

/** libclang 可能所在的目录（静态候选）/ Static candidate directories for libclang */
function libclangDirCandidates() {
  const dirs = [];

  if (process.env.LIBCLANG_PATH) dirs.push(process.env.LIBCLANG_PATH);

  if (process.env.PREFIX) dirs.push(path.join(process.env.PREFIX, "lib")); // Termux

  if (isMac) {
    dirs.push(
      "/opt/homebrew/opt/llvm/lib",
      "/usr/local/opt/llvm/lib",
      "/opt/homebrew/lib",
      "/usr/local/lib",
      "/Library/Developer/CommandLineTools/usr/lib",
      "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib"
    );
  } else if (isWindows) {
    dirs.push(
      "C:\\Program Files\\LLVM\\bin",
      "C:\\Program Files (x86)\\LLVM\\bin",
      path.join(process.env.ProgramFiles || "C:\\Program Files", "LLVM", "bin")
    );
  } else {
    // llvm 版本化安装目录 / versioned llvm installs
    for (const root of ["/usr/lib", "/usr/local/lib", "/opt"]) {
      for (const llvm of globDirs(path.join(root, "llvm-*"))) {
        dirs.push(path.join(llvm, "lib"), path.join(llvm, "lib64"));
      }
    }
    // 发行版多架构目录 / distro multiarch directories
    for (const arch of [
      "x86_64-linux-gnu", "aarch64-linux-gnu", "arm-linux-gnueabihf",
      "i386-linux-gnu", "riscv64-linux-gnu",
    ]) {
      dirs.push(`/usr/lib/${arch}`, `/usr/local/lib/${arch}`, `/lib/${arch}`);
    }
    dirs.push("/usr/lib", "/usr/lib64", "/usr/local/lib", "/lib", "/lib64");
  }

  // pip 的 clang wheel（无需 root，proot/Termux 里最省事）
  // the pip `clang` wheel (no root needed; easiest inside proot/Termux)
  const pyLibRoots = [
    "/usr/local/lib", "/usr/lib", "/usr/local/local/lib",
    path.join(os.homedir(), ".local", "lib"),
  ];
  for (const root of pyLibRoots) {
    for (const py of globDirs(path.join(root, "python3*"))) {
      for (const sp of ["dist-packages", "site-packages"]) {
        dirs.push(path.join(py, sp, "clang", "native"));
      }
    }
  }

  return [...new Set(dirs)];
}

/** 用 python 询问 pip 版 clang 的原生库目录 / Ask python for the pip clang native dir */
function libclangFromPython() {
  const code =
    "import os,clang;print(os.path.join(os.path.dirname(clang.__file__),'native'))";
  for (const py of ["python3", "python", "py"]) {
    if (!which(py)) continue;
    try {
      const out = execFileSync(py, ["-c", code], {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
        timeout: 20000,
      }).trim();
      if (out && hasLibclang(out)) return out;
    } catch {
      /* 该解释器没有 clang 包 / this interpreter lacks the clang package */
    }
  }
  return null;
}

/** 用 llvm-config 询问 libdir / Ask llvm-config for its libdir */
function libclangFromLlvmConfig() {
  const exe = which("llvm-config");
  if (!exe) return null;
  try {
    const out = execFileSync(exe, ["--libdir"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
      timeout: 20000,
    }).trim();
    return hasLibclang(out) ? out : null;
  } catch {
    return null;
  }
}

/** 尝试通过 pip 安装 clang wheel / Try installing the clang wheel via pip */
function installLibclang() {
  const attempts = [
    ["python3", ["-m", "pip", "install", "--quiet", "--disable-pip-version-check", "libclang"]],
    ["pip3", ["install", "--quiet", "--disable-pip-version-check", "libclang"]],
    ["python3", ["-m", "pip", "install", "--quiet", "--user", "--disable-pip-version-check", "libclang"]],
  ];

  for (const [cmd, args] of attempts) {
    if (!which(cmd)) continue;
    process.stdout.write(
      "    libclang not found — installing the `libclang` wheel " +
      "(needed by bindgen for the TLS-fingerprinting dependency)…\n"
    );
    try {
      execFileSync(cmd, args, { stdio: "inherit", timeout: 300000 });
      return true;
    } catch {
      /* 换下一个方式 / try the next approach */
    }
  }
  return false;
}

/**
 * 解析 libclang 所在目录。
 * Resolve the directory that contains the libclang shared library.
 *
 * @param {boolean} allowInstall 允许自动安装 / allow auto-install
 * @returns {string|null}
 */
function resolveLibclang(allowInstall = true) {
  if (hasLibclang(process.env.LIBCLANG_PATH)) return process.env.LIBCLANG_PATH;

  const staticHit = libclangDirCandidates().find(hasLibclang);
  if (staticHit) return staticHit;

  const pyHit = libclangFromPython();
  if (pyHit) return pyHit;

  const llvmHit = libclangFromLlvmConfig();
  if (llvmHit) return llvmHit;

  if (allowInstall && installLibclang()) {
    const afterInstall =
      libclangDirCandidates().find(hasLibclang) || libclangFromPython();
    if (afterInstall) return afterInstall;
  }
  return null;
}

/** gcc/clang 系统头文件目录（pip 版 libclang 默认看不到它们）
 *  System include directories (the pip libclang does not find them by itself) */
function systemIncludeArgs() {
  if (isWindows || isMac) return [];

  const dirs = [];
  // gcc 自带头文件 / gcc's own headers
  for (const gcc of globDirs("/usr/lib/gcc/*/*")) {
    const inc = path.join(gcc, "include");
    if (fs.existsSync(inc)) dirs.push(inc);
  }
  for (const arch of ["aarch64-linux-gnu", "x86_64-linux-gnu", "arm-linux-gnueabihf"]) {
    const inc = path.join("/usr/include", arch);
    if (fs.existsSync(inc)) dirs.push(inc);
  }
  if (process.env.PREFIX) {
    const inc = path.join(process.env.PREFIX, "include");
    if (fs.existsSync(inc)) dirs.push(inc);
  }
  if (fs.existsSync("/usr/include")) dirs.push("/usr/include");

  return dirs.map((d) => `-I${d}`);
}

// ── 入口 / Entry point ───────────────────────────────────────────────────────

/**
 * 准备构建环境：cargo 在 PATH 上、LIBCLANG_PATH 指向 libclang。
 * 失败时打印可执行的修复说明并退出，而不是让 cargo 抛出一大段栈。
 *
 * Prepare the build environment: cargo on PATH and LIBCLANG_PATH pointing at
 * libclang. On failure, print actionable instructions and exit instead of
 * letting cargo dump a long stack trace.
 *
 * @param {{ quiet?: boolean }} [opts]
 */
function prepareBuildEnv(opts = {}) {
  const log = opts.quiet ? () => {} : (msg) => console.log(msg);

  // ── cargo ──
  if (!ensureCargo()) {
    console.error(`
  ✗ 找不到 cargo / cargo not found

    需要 Rust 工具链 / A Rust toolchain is required:
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
      source "$HOME/.cargo/env"

    装好后重试 / Then re-run:  npm run dev
`);
    process.exit(1);
  }

  // ── libclang ──
  const alreadySet = hasLibclang(process.env.LIBCLANG_PATH);
  const dir = resolveLibclang(true);
  if (!dir) {
    console.error(`
  ✗ 找不到 libclang / libclang not found

    后端依赖的 BoringSSL（btls-sys）在编译期用 bindgen 生成绑定，需要 libclang。
    The backend's BoringSSL dependency (btls-sys) uses bindgen at build time and
    needs the libclang shared library.

      Debian/Ubuntu : sudo apt-get install -y libclang-dev clang
      Alpine        : sudo apk add clang-dev
      Termux        : pkg install clang
      macOS         : brew install llvm
      任意系统(免 root) / anywhere (no root): python3 -m pip install libclang

    如果已经装好但位置特殊，手动指定目录（含 libclang.so）：
    If it is installed in an unusual place, point at its directory:
      LIBCLANG_PATH=/path/to/lib npm run dev
`);
    process.exit(1);
  }

  process.env.LIBCLANG_PATH = dir;
  // 只在 DEBUG 下打印，避免每次构建都在标题前多一行噪声
  // Only under DEBUG, so a normal build does not add a line above the header.
  if (!alreadySet && process.env.DEBUG === "1") log(`    libclang: ${dir}`);

  // bindgen 用 pip 版 libclang 时看不到系统头文件，显式补上
  // The pip libclang cannot see the system headers by itself, so add them.
  if (!process.env.BINDGEN_EXTRA_CLANG_ARGS) {
    const incs = systemIncludeArgs();
    if (incs.length) process.env.BINDGEN_EXTRA_CLANG_ARGS = incs.join(" ");
  }
}

module.exports = {
  prepareBuildEnv,
  resolveLibclang,
  ensureCargo,
  hasLibclang,
};
