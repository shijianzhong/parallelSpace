//! 用户会话与邮箱验证码（G7 定版，见 docs/GAPS_AND_PLAN.md 第六节）。
//!
//! - 会话：KV `session:{token}` → account_id（text 存取配对），token 为双 ULID
//!   拼接（约 160 位熵），Cookie `ps_session`（HttpOnly/Secure/SameSite=Lax）
//!   或 `Authorization: Bearer` 均可携带；TTL 30 天。
//! - OTP：6 位码，KV `otp:{email}` 存哈希（FNV-1a + pepper），TTL 10 分钟，
//!   最多试 5 次，发送间隔 60 秒。发信走 Resend（secret RESEND_API_KEY），
//!   未配置时为日志模式（仅本地开发，验证码打到 worker 日志）。
//! - 与 admin token 体系完全分开：这里只认普通用户会话。

use serde::{Deserialize, Serialize};
use worker::{Env, Fetch, Method, RequestInit};

use crate::common::error::{AppError, AppResult};

pub const SESSION_COOKIE: &str = "ps_session";
const SESSION_TTL_SECS: u64 = 30 * 24 * 3600;
const OTP_TTL_SECS: u64 = 600;
const OTP_MAX_ATTEMPTS: u32 = 5;

/// FNV-1a 64 位（与 llm/cache::prompt_hash 同算法，本地实现避免跨模块耦合）。
fn fnv1a(s: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(PRIME);
    }
    format!("{h:016x}")
}

fn pepper(env: &Env) -> String {
    env.secret("ADMIN_TOKEN")
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "dev-pepper".into())
}

/// 签发会话，返回 token。
pub async fn issue_session(env: &Env, account_id: &str) -> AppResult<String> {
    let token = format!(
        "{}{}",
        ulid::Ulid::generate(),
        ulid::Ulid::generate()
    );
    let kv = env.kv("KV")?;
    kv.put(&format!("session:{token}"), account_id)?
        .expiration_ttl(SESSION_TTL_SECS)
        .execute()
        .await?;
    Ok(token)
}

/// 从请求解析会话（Bearer 优先，其次 Cookie）。无会话返回 None（不报错，
/// 供「有会话则校验归属」的过渡逻辑使用）。
pub async fn resolve_session(req: &worker::Request, env: &Env) -> AppResult<Option<String>> {
    let token = extract_token(req);
    let Some(token) = token else {
        return Ok(None);
    };
    let kv = env.kv("KV")?;
    let account = kv.get(&format!("session:{token}")).text().await?;
    Ok(account)
}

/// 强制会话：无/失效即 401。
pub async fn require_session(req: &worker::Request, env: &Env) -> AppResult<String> {
    resolve_session(req, env)
        .await?
        .ok_or(AppError::Unauthorized)
}

fn extract_token(req: &worker::Request) -> Option<String> {
    if let Ok(Some(auth)) = req.headers().get("authorization") {
        if let Some(t) = auth.strip_prefix("Bearer ") {
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    if let Ok(Some(cookie)) = req.headers().get("cookie") {
        for part in cookie.split(';') {
            let part = part.trim();
            if let Some(v) = part.strip_prefix(&format!("{SESSION_COOKIE}=")) {
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

pub fn session_cookie(token: &str) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={SESSION_TTL_SECS}"
    )
}

/// 归属校验（过渡模式）：带了会话就必须与分身归属一致；没带会话时，
/// 仅当部署设置 REQUIRE_AUTH=true 才拒绝（试运行部署打开，本地 smoke 不受影响）。
pub async fn enforce_ownership(req: &worker::Request, env: &Env, agent_id: &str) -> AppResult<()> {
    if let Some(account_id) = resolve_session(req, env).await? {
        let db = env.d1("DB")?;
        let agent = crate::storage::d1::get_agent(&db, agent_id)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("agent {agent_id}")))?;
        if agent.account_id != account_id {
            return Err(AppError::Forbidden);
        }
        return Ok(());
    }
    let require = env
        .var("REQUIRE_AUTH")
        .map(|v| v.to_string() == "true")
        .unwrap_or(false);
    if require {
        return Err(AppError::Unauthorized);
    }
    Ok(())
}

// ---------- 邮箱验证码 ----------

#[derive(Debug, Serialize, Deserialize)]
struct OtpRecord {
    hash: String,
    attempts: u32,
}

pub enum OtpStart {
    Sent,
    /// 未配置发信通道的日志模式（本地开发）：调用方决定是否回显验证码
    LogOnly(String),
}

/// 生成并发送验证码。发送间隔 60 秒（重复请求 409）。
pub async fn start_otp(env: &Env, email: &str) -> AppResult<OtpStart> {
    let kv = env.kv("KV")?;
    let rl_key = format!("otp_rl:{email}");
    if kv.get(&rl_key).text().await?.is_some() {
        return Err(AppError::Conflict("otp requested too frequently".into()));
    }
    kv.put(&rl_key, "1")?
        .expiration_ttl(60)
        .execute()
        .await?;

    let raw = ulid::Ulid::generate().0;
    let code = format!("{:06}", raw % 900_000 + 100_000);
    let record = OtpRecord {
        hash: fnv1a(&format!("{email}:{code}:{}", pepper(env))),
        attempts: 0,
    };
    kv.put(
        &format!("otp:{email}"),
        serde_json::to_string(&record).map_err(|e| AppError::Internal(e.to_string()))?,
    )?
    .expiration_ttl(OTP_TTL_SECS)
    .execute()
    .await?;

    match send_otp_email(env, email, &code).await? {
        true => Ok(OtpStart::Sent),
        false => Ok(OtpStart::LogOnly(code)),
    }
}

/// 校验验证码：错误累计 attempts，超限作废；成功即删除（防重放）。
pub async fn verify_otp(env: &Env, email: &str, code: &str) -> AppResult<()> {
    let kv = env.kv("KV")?;
    let key = format!("otp:{email}");
    let raw = kv
        .get(&key)
        .text()
        .await?
        .ok_or_else(|| AppError::InvalidInput("otp expired or not requested".into()))?;
    let mut record: OtpRecord = serde_json::from_str(&raw)
        .map_err(|e| AppError::Internal(format!("otp record corrupt: {e}")))?;
    if record.attempts >= OTP_MAX_ATTEMPTS {
        return Err(AppError::Forbidden);
    }
    let expect = fnv1a(&format!("{email}:{code}:{}", pepper(env)));
    if expect != record.hash {
        record.attempts += 1;
        kv.put(&key, serde_json::to_string(&record).map_err(|e| AppError::Internal(e.to_string()))?)?
            .expiration_ttl(OTP_TTL_SECS)
            .execute()
            .await?;
        return Err(AppError::Unauthorized);
    }
    kv.delete(&key).await?;
    Ok(())
}

/// 发信：配置了 RESEND_API_KEY 走 Resend API（返回 true）；
/// 未配置则日志模式（返回 false），验证码仅写 worker 日志，绝不进响应，
/// 除非部署显式设置 ALLOW_DEV_OTP_ECHO=true（只应出现在本地 .dev.vars）。
async fn send_otp_email(env: &Env, email: &str, code: &str) -> AppResult<bool> {
    let api_key = match env.secret("RESEND_API_KEY") {
        Ok(k) => k.to_string(),
        Err(_) => {
            worker::console_log!("OTP for {email}: {code} (log-only mode, RESEND_API_KEY unset)");
            return Ok(false);
        }
    };
    let from = env
        .var("MAIL_FROM")
        .map(|v| v.to_string())
        .unwrap_or_else(|_| "ParallelSpace <onboarding@resend.dev>".into());
    let body = serde_json::json!({
        "from": from,
        "to": [email],
        "subject": "ParallelSpace 登录验证码",
        "text": format!("你的验证码是 {code}，10 分钟内有效。如非本人操作请忽略。"),
    });
    let headers = worker::Headers::new();
    headers.set("authorization", &format!("Bearer {api_key}"))?;
    headers.set("content-type", "application/json")?;
    let mut init = RequestInit::new();
    init.method = Method::Post;
    init.headers = headers;
    let bytes = worker::js_sys::Uint8Array::from(serde_json::to_string(&body).map_err(|e| AppError::Internal(e.to_string()))?.as_bytes());
    init.body = Some(bytes.into());
    let req = worker::Request::new_with_init("https://api.resend.com/emails", &init)?;
    let mut resp = Fetch::Request(req).send().await?;
    if resp.status_code() != 200 && resp.status_code() != 201 {
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Internal(format!(
            "resend send failed: {} {text}",
            resp.status_code()
        )));
    }
    Ok(true)
}

/// dev 回显是否开启（本地 smoke 用）：日志模式 + 显式开关，生产两者不可兼得。
pub fn dev_echo_allowed(env: &Env, log_only: bool) -> bool {
    log_only
        && env
            .var("ALLOW_DEV_OTP_ECHO")
            .map(|v| v.to_string() == "true")
            .unwrap_or(false)
}
