//! 交互式登录会话管理 / Interactive Login Session Manager
//!
//! 某些模块需要人工完成一次登录（例如 Telegram 用户账号需要手机号验证码），
//! 这类登录无法在单次进程调用内完成，因此这里让模块进程常驻，
//! 通过 stdin/stdout 行协议分步驱动，并把中间状态保存在内存中。
//!
//! Some modules need a one-time interactive sign-in (e.g. a Telegram user account
//! requires a phone number and a login code). That cannot be done in a single process
//! invocation, so the module process is kept alive and driven step by step over a
//! line protocol on stdin/stdout, with the intermediate state kept in memory.
//!
//! # 协议 / Protocol
//! - 宿主 → 模块（stdin）：`命令:值`，例如 `phone:+8613800138000`、`code:12345`、`password:…`、`quit`
//! - 模块 → 宿主（stdout）：以 `LOGIN_STATE:` 开头的行，例如 `LOGIN_STATE:code_sent`
//! - Host → module (stdin): `command:value`, e.g. `phone:+8613800138000`, `code:12345`, `password:…`, `quit`
//! - Module → host (stdout): lines starting with `LOGIN_STATE:`, e.g. `LOGIN_STATE:code_sent`

use crate::config::settings::exe_dir;
use crate::postprocess::pipeline::ModuleInfo;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 模块输出中表示登录状态的行前缀 / Prefix of module stdout lines that carry login state.
const LOGIN_STATE_PREFIX: &str = "LOGIN_STATE:";

/// 单个登录步骤（发码、提交验证码、提交密码）的最长等待时间。
/// Maximum time to wait for one login step (request code, submit code, submit password).
const LOGIN_STEP_TIMEOUT: Duration = Duration::from_secs(180);

/// 登录会话空闲超时：超过该时间没有交互就结束进程，避免进程堆积。
/// Idle timeout: a session with no interaction for this long is terminated.
const LOGIN_IDLE_TIMEOUT: Duration = Duration::from_secs(900);

/// 每个会话保留的日志行数上限 / Maximum number of log lines kept per session.
const LOGIN_MAX_LOG_LINES: usize = 300;

/// 登录过程的一行日志 / One line of login output.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginLog {
    /// 输出流（"stdout" / "stderr"）/ Output stream ("stdout" / "stderr")
    pub stream: String,
    /// 行内容 / Line content
    pub line: String,
}

/// 一次登录交互的结果 / Result of one login interaction.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginUpdate {
    /// 登录会话 ID / Login session id
    pub login_id: String,
    /// 当前状态（ready / code_sent / password_required / invalid_code / invalid_password /
    /// done / already / error / cancelled）
    /// Current state
    pub state: String,
    /// 状态附加信息（两步验证密码提示、账号名、错误信息等）
    /// Extra state detail (2FA hint, account name, error message, …)
    pub detail: String,
    /// 会话文件路径（模块报告时）/ Session file path (when reported by the module)
    pub session_path: Option<String>,
    /// 本次交互产生的日志 / Logs produced by this interaction
    pub logs: Vec<LoginLog>,
}

impl LoginUpdate {
    /// 是否为终止状态（登录成功、已登录、失败或取消）。
    /// Whether the state is terminal (signed in, already signed in, failed or cancelled).
    fn is_terminal(&self) -> bool {
        matches!(self.state.as_str(), "done" | "already" | "error" | "cancelled")
    }
}

/// 子进程输出事件 / Subprocess output events
enum Event {
    /// 一行输出（是否来自 stderr，内容）/ One output line (is stderr, content)
    Line(bool, String),
    /// 某个输出流结束 / One output stream reached EOF
    Eof,
}

/// 常驻的登录子进程及其状态 / Resident login child process and its state.
struct LoginSession {
    /// 子进程句柄 / Child process handle
    child: Child,
    /// 子进程 stdin（写入命令）/ Child stdin (commands are written here)
    stdin: Option<ChildStdin>,
    /// 子进程输出事件接收端 / Receiver of subprocess output events
    rx: Receiver<Event>,
    /// 尚未返回给调用方的日志行 / Log lines not yet returned to the caller
    logs: Vec<LoginLog>,
    /// 模块报告的会话文件路径 / Session file path reported by the module
    session_path: Option<String>,
    /// 最近一次交互时间（用于空闲清理）/ Last interaction time (used for idle cleanup)
    last_active: Instant,
}

/// 全局登录会话表 / Global login session registry.
fn sessions() -> &'static Mutex<HashMap<String, Arc<Mutex<LoginSession>>>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Arc<Mutex<LoginSession>>>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 生成一个登录会话 ID / Generate a login session id.
fn new_login_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    format!("{:x}{:04x}", nanos, COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// 结束并回收一个登录子进程 / Terminate and reap a login child process.
fn shutdown(session: &mut LoginSession) {
    // 先关闭 stdin，让模块在读取时收到 EOF 后自行退出
    // Close stdin first so the module sees EOF and exits on its own
    session.stdin.take();
    // 进程可能已经退出，kill 失败无妨；wait 确保回收，不留僵尸进程
    // The process may have exited already; a failing kill is fine, and wait reaps it so no
    // zombie is left behind
    let _ = session.child.kill();
    let _ = session.child.wait();
}

/// 清理空闲超时的登录会话 / Drop login sessions that have been idle for too long.
fn cleanup_idle() {
    let mut map = match sessions().lock() {
        Ok(m) => m,
        Err(e) => e.into_inner(),
    };
    let expired: Vec<String> = map
        .iter()
        .filter(|(_, s)| match s.lock() {
            Ok(s) => s.last_active.elapsed() > LOGIN_IDLE_TIMEOUT,
            Err(e) => e.into_inner().last_active.elapsed() > LOGIN_IDLE_TIMEOUT,
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in expired {
        if let Some(session) = map.remove(&id) {
            let mut guard = match session.lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            shutdown(&mut guard);
        }
        tracing::info!("login session {} expired after inactivity", id);
    }
}

/// 把一行输出解析为 `(状态, 附加信息)`；非状态行返回 `None`。
/// Parse one output line into `(state, detail)`; returns `None` for non-state lines.
fn parse_state_line(line: &str) -> Option<(String, String)> {
    let rest = line.trim().strip_prefix(LOGIN_STATE_PREFIX)?;
    let (state, detail) = match rest.split_once(':') {
        Some((s, d)) => (s.trim().to_string(), d.trim().to_string()),
        None => (rest.trim().to_string(), String::new()),
    };
    if state.is_empty() {
        return None;
    }
    Some((state, detail))
}

/// 收集子进程输出，直到出现一条 `LOGIN_STATE:` 状态行为止。
///
/// Collect subprocess output until a `LOGIN_STATE:` line shows up.
///
/// # 参数 / Parameters
/// - `session`: 登录会话 / Login session
/// - `login_id`: 会话 ID（写入返回值）/ Session id (copied into the result)
fn wait_for_state(
    session: &mut LoginSession,
    login_id: &str,
) -> Result<LoginUpdate, String> {
    let started = Instant::now();
    let mut logs = std::mem::take(&mut session.logs);
    let mut eof_count = 0usize;

    loop {
        if started.elapsed() > LOGIN_STEP_TIMEOUT {
            shutdown(session);
            return Err(format!(
                "登录超时（{} 秒没有响应）",
                LOGIN_STEP_TIMEOUT.as_secs()
            ));
        }

        match session.rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Line(is_stderr, line)) => {
                let line = line.trim_end().to_string();
                if line.trim().is_empty() {
                    continue;
                }
                if let Some((state, detail)) = parse_state_line(&line) {
                    if state == "session" {
                        // 会话文件路径单独保存，不作为一次交互的结果
                        // The session path is stored separately, not as an interaction result
                        session.session_path = Some(detail);
                        continue;
                    }
                    session.last_active = Instant::now();
                    return Ok(LoginUpdate {
                        login_id: login_id.to_string(),
                        state,
                        detail,
                        session_path: session.session_path.clone(),
                        logs,
                    });
                }
                if logs.len() < LOGIN_MAX_LOG_LINES {
                    logs.push(LoginLog {
                        stream: if is_stderr { "stderr" } else { "stdout" }.to_string(),
                        line,
                    });
                }
            }
            Ok(Event::Eof) => {
                eof_count += 1;
                if eof_count >= 2 {
                    // 两个流都结束：进程已退出但没有给出状态
                    // Both streams ended: the process exited without reporting a state
                    let status = session.child.wait().ok();
                    let code = status
                        .and_then(|s| s.code())
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    let last_err = logs
                        .iter()
                        .rev()
                        .find(|l| l.stream == "stderr")
                        .map(|l| l.line.clone())
                        .unwrap_or_else(|| "模块没有输出任何信息".to_string());
                    return Ok(LoginUpdate {
                        login_id: login_id.to_string(),
                        state: "error".to_string(),
                        detail: format!("{}（退出码 {}）", last_err, code),
                        session_path: session.session_path.clone(),
                        logs,
                    });
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err("登录进程已断开".to_string());
            }
        }
    }
}

/// 启动一次交互式登录（模块以 `--login` 参数运行，进程保持存活）。
///
/// Start an interactive login (the module runs with `--login` and stays alive).
///
/// # 参数 / Parameters
/// - `module`: 模块信息（需支持 `loginable`）/ Module info (must be `loginable`)
/// - `params`: 节点参数值（传给模块的环境变量）/ Node parameter values (passed as env vars)
/// - `force`: 是否强制全新登录（清除已有的会话文件，重新输入手机号）/ Whether to force fresh login
pub fn start_login(
    module: &ModuleInfo,
    params: &HashMap<String, serde_json::Value>,
    force: bool,
) -> Result<LoginUpdate, String> {
    if !module.loginable {
        return Err(format!("模块 '{}' 不支持登录", module.id));
    }

    cleanup_idle();

    let mut cmd = Command::new(&module.exe_path);
    cmd.arg("--login");
    if force {
        cmd.arg("--fresh");
        cmd.env("PP_FORCE_LOGIN", "1");
        cmd.env("PP_PARAM_FORCE_LOGIN", "1");
    }
    cmd.env("PP_EXE_DIR", exe_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (key, val) in params {
        let env_val = match val {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        cmd.env(format!("PP_PARAM_{}", key.to_uppercase()), env_val);
    }

    let mut child = cmd.spawn().map_err(|e| format!("启动失败: {}", e))?;
    let stdin = child.stdin.take();
    let (tx, rx) = mpsc::channel::<Event>();

    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(Event::Line(false, line)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Event::Eof);
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx.send(Event::Line(true, line)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Event::Eof);
        });
    }
    drop(tx);

    let mut session = LoginSession {
        child,
        stdin,
        rx,
        logs: Vec::new(),
        session_path: None,
        last_active: Instant::now(),
    };

    let login_id = new_login_id();
    let update = wait_for_state(&mut session, &login_id)?;

    // 已登录 / 出错 / 取消：进程已结束，无需保留会话
    // Already signed in / failed / cancelled: the process is done, nothing to keep
    if update.is_terminal() {
        shutdown(&mut session);
        return Ok(update);
    }

    let mut map = match sessions().lock() {
        Ok(m) => m,
        Err(e) => e.into_inner(),
    };
    map.insert(login_id.clone(), Arc::new(Mutex::new(session)));
    Ok(update)
}

/// 向登录会话写入一行命令，并返回下一个状态。
///
/// Write one command line to a login session and return the next state.
///
/// # 参数 / Parameters
/// - `login_id`: `start_login` 返回的会话 ID / Session id returned by `start_login`
/// - `value`: 命令行（如 `phone:+8613800138000`）/ Command line (e.g. `phone:+8613800138000`)
pub fn send_login_input(login_id: &str, value: &str) -> Result<LoginUpdate, String> {
    let session = {
        let map = match sessions().lock() {
            Ok(m) => m,
            Err(e) => e.into_inner(),
        };
        map.get(login_id).cloned()
    };
    let session = session.ok_or_else(|| "登录会话不存在或已结束".to_string())?;

    let mut guard = match session.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };

    guard.last_active = Instant::now();

    {
        let stdin = guard
            .stdin
            .as_mut()
            .ok_or_else(|| "登录进程已结束".to_string())?;
        writeln!(stdin, "{}", value).map_err(|e| format!("写入登录进程失败: {}", e))?;
        stdin.flush().map_err(|e| format!("写入登录进程失败: {}", e))?;
    }

    let update = wait_for_state(&mut guard, login_id)?;

    if update.is_terminal() {
        shutdown(&mut guard);
        drop(guard);
        if let Ok(mut map) = sessions().lock() {
            map.remove(login_id);
        }
    }

    Ok(update)
}

/// 取消并结束一个登录会话 / Cancel and terminate a login session.
pub fn cancel_login(login_id: &str) -> Result<(), String> {
    let session = {
        let mut map = match sessions().lock() {
            Ok(m) => m,
            Err(e) => e.into_inner(),
        };
        map.remove(login_id)
    };
    if let Some(session) = session {
        let mut guard = match session.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        shutdown(&mut guard);
    }
    Ok(())
}

/// 统计当前存活的登录会话数量（调试用）/ Count live login sessions (for diagnostics).
pub fn active_session_count() -> usize {
    match sessions().lock() {
        Ok(m) => m.len(),
        Err(e) => e.into_inner().len(),
    }
}

// ─── 单元测试 / Unit tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_state_lines_with_and_without_detail() {
        assert_eq!(
            parse_state_line("LOGIN_STATE:code_sent"),
            Some(("code_sent".to_string(), String::new()))
        );
        assert_eq!(
            parse_state_line("LOGIN_STATE:done:@alice:premium"),
            Some(("done".to_string(), "@alice:premium".to_string()))
        );
        assert_eq!(
            parse_state_line("LOGIN_STATE:password_required:pet name"),
            Some(("password_required".to_string(), "pet name".to_string()))
        );
    }

    #[test]
    fn ignores_non_state_lines() {
        assert_eq!(parse_state_line("connecting…"), None);
        assert_eq!(parse_state_line("LOGIN_STATE:"), None);
        assert_eq!(parse_state_line("LOGIN_STATE:   "), None);
        assert_eq!(parse_state_line(""), None);
    }

    #[test]
    fn terminal_states_end_the_session() {
        let make = |state: &str| LoginUpdate {
            login_id: "id".to_string(),
            state: state.to_string(),
            detail: String::new(),
            session_path: None,
            logs: vec![],
        };
        for state in ["done", "already", "error", "cancelled"] {
            assert!(make(state).is_terminal(), "{} should be terminal", state);
        }
        for state in ["ready", "code_sent", "password_required", "invalid_code"] {
            assert!(!make(state).is_terminal(), "{} should not be terminal", state);
        }
    }

    #[test]
    fn login_ids_are_unique() {
        let a = new_login_id();
        let b = new_login_id();
        assert_ne!(a, b);
    }
}

/// 刷新 Telegram 会话（非交互式）。
///
/// 运行模块的 `--refresh` 参数：重新打开已授权的用户会话，强制同步 peer 缓存
/// 并刷新认证密钥。不需要手机号/验证码；会话未授权时会返回明确的错误提示。
///
/// Refresh a Telegram session (non-interactive).
///
/// Runs the module with `--refresh`: re-opens an authorized user session,
/// force-syncs the peer cache, and refreshes the auth key. No phone number/
/// verification code needed; if the session is not authorized, returns a clear
/// error hint instead of leaving the user guessing.
///
/// Returns `RefreshStatus::Ok` when the session refreshed successfully, `RefreshStatus::NotAuthorized`
/// when the session file exists but is not signed in, or `RefreshStatus::Error(String)` otherwise.
pub fn refresh_session(module: &ModuleInfo, api_id: i32, proxy: &str) -> Result<RefreshStatus, String> {
    if !module.loginable {
        return Err(format!("模块 '{}' 不支持登录", module.id));
    }

    let mut cmd = Command::new(&module.exe_path);
    cmd.arg("--refresh")
        .env("PP_EXE_DIR", exe_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("api_id", api_id.to_string())
        .env("proxy", proxy);

    let mut child = cmd.spawn().map_err(|e| format!("启动刷新失败: {}", e))?;
    let (tx, rx) = mpsc::channel::<Event>();

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    if let Some(stdout) = stdout {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(Event::Line(false, line)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Event::Eof);
        });
    }
    if let Some(stderr) = stderr {
        let tx_s = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx_s.send(Event::Line(true, line)).is_err() {
                    return;
                }
            }
            let _ = tx_s.send(Event::Eof);
        });
    }
    drop(tx);

    let mut result = RefreshStatus::Ok(String::new());
    loop {
        match rx.recv() {
            Ok(Event::Line(false, line)) => {
                if let Some(stripped) = line.strip_prefix("LOGIN_STATE:") {
                    if stripped.starts_with("refreshed:") {
                        result = RefreshStatus::Ok(stripped["refreshed:".len()..].to_string());
                        break;
                    }
                }
            }
            Ok(Event::Line(true, line)) => {
                if !line.is_empty() {
                    result = RefreshStatus::Error(line);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
        }
    }

    if let Err(e) = child.wait() {
        if let RefreshStatus::Ok(_) = result {
            result = RefreshStatus::Error(format!("刷新进程退出失败: {}", e));
        }
    }

    Ok(result)
}

/// 刷新会话的结果 / Refresh session result
#[derive(Debug, Clone)]
pub enum RefreshStatus {
    /// 成功，值为刷新后的用户摘要
    /// Success; the value is the refreshed user summary
    Ok(String),
    /// 会话未授权
    /// Session exists but is not authorized
    NotAuthorized,
    /// 错误
    /// Error
    Error(String),
}

impl RefreshStatus {
    pub fn is_ok(&self) -> bool {
        matches!(self, RefreshStatus::Ok(_))
    }

    pub fn message(&self) -> String {
        match self {
            RefreshStatus::Ok(user) => format!("已刷新会话：{}", user),
            RefreshStatus::NotAuthorized => "会话文件存在但未登录（未授权）。请先登录 Telegram 账号".to_string(),
            RefreshStatus::Error(e) => e.clone(),
        }
    }
}
