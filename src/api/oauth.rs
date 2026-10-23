//! Google / GitHub OAuth 登录（G7 通道二）。
//!
//! 流程：GET /api/auth/oauth/:provider/start → 302 到授权页（state 存 KV 防 CSRF）
//! → GET /api/auth/oauth/:provider/callback 换 token、取 sub+已验证邮箱 →
//! resolve_or_create_account（同邮箱自动关联）→ 下发会话 Cookie 并 302 回 APP_BASE_URL。
//! 密钥走 worker secret：GOOGLE_CLIENT_ID / GOOGLE_CLIENT_SECRET /
//! GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET；未配置的 provider 返回明确错误，不静默降级。

use worker::{Fetch, Method, RequestInit, Response, RouteContext};

use crate::api::auth::{resolve_or_create_account, respond_with_session};
use crate::common::error::{AppError, AppResult};

struct ProviderCfg {
    auth_url: &'static str,
    token_url: &'static str,
    userinfo_url: &'static str,
    scope: &'static str,
    id_var: &'static str,
    secret_var: &'static str,
}

fn cfg(provider: &str) -> AppResult<ProviderCfg> {
    match provider {
        "google" => Ok(ProviderCfg {
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            userinfo_url: "https://openidconnect.googleapis.com/v1/userinfo",
            scope: "openid email profile",
            id_var: "GOOGLE_CLIENT_ID",
            secret_var: "GOOGLE_CLIENT_SECRET",
        }),
        "github" => Ok(ProviderCfg {
            auth_url: "https://github.com/login/oauth/authorize",
            token_url: "https://github.com/login/oauth/access_token",
            userinfo_url: "https://api.github.com/user",
            scope: "read:user user:email",
            id_var: "GITHUB_CLIENT_ID",
            secret_var: "GITHUB_CLIENT_SECRET",
        }),
        other => Err(AppError::InvalidInput(format!(
            "unknown oauth provider: {other}"
        ))),
    }
}

fn client_pair(env: &worker::Env, c: &ProviderCfg) -> AppResult<(String, String)> {
    let id = env
        .var(c.id_var)
        .map(|v| v.to_string())
        .map_err(|_| AppError::Internal(format!("{} not configured", c.id_var)))?;
    let secret = env
        .secret(c.secret_var)
        .map(|s| s.to_string())
        .map_err(|_| AppError::Internal(format!("{} not configured", c.secret_var)))?;
    Ok((id, secret))
}

fn redirect_uri(env: &worker::Env, provider: &str) -> String {
    let base = env
        .var("APP_BASE_URL")
        .map(|v| v.to_string())
        .unwrap_or_else(|_| "http://localhost:8787".into());
    format!("{base}/api/auth/oauth/{provider}/callback")
}

fn urlencode(s: &str) -> String {
    // 够用的百分号编码（client_id/secret/uri 只含 ASCII 保留字）
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 发起授权：GET /api/auth/oauth/:provider/start。
pub async fn start(req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let provider = ctx
        .param("provider")
        .map(|s| s.to_string())
        .unwrap_or_default();
    let c = cfg(&provider)?;
    let env = ctx.env.clone();
    let (client_id, _) = client_pair(&env, &c)?;
    let state = ulid::Ulid::generate().to_string();
    let kv = env.kv("KV")?;
    kv.put(&format!("oauth_state:{state}"), provider.as_str())?
        .expiration_ttl(600)
        .execute()
        .await?;
    let _ = req;
    let url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
        c.auth_url,
        urlencode(&client_id),
        urlencode(&redirect_uri(&env, &provider)),
        urlencode(c.scope),
        state
    );
    Ok(Response::redirect(url.parse().map_err(|_| {
        AppError::Internal("bad authorize url".into())
    })?)?)
}

/// 授权回调：GET /api/auth/oauth/:provider/callback?code=&state=。
pub async fn callback(req: worker::Request, ctx: RouteContext<()>) -> AppResult<Response> {
    let provider = ctx
        .param("provider")
        .map(|s| s.to_string())
        .unwrap_or_default();
    let c = cfg(&provider)?;
    let env = ctx.env.clone();
    let url = req.url()?;
    let mut code = String::new();
    let mut state = String::new();
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = v.into_owned(),
            "state" => state = v.into_owned(),
            _ => {}
        }
    }
    if code.is_empty() || state.is_empty() {
        return Err(AppError::InvalidInput("missing code/state".into()));
    }
    let kv = env.kv("KV")?;
    let state_key = format!("oauth_state:{state}");
    let stored = kv.get(&state_key).text().await?;
    if stored.as_deref() != Some(provider.as_str()) {
        return Err(AppError::Forbidden);
    }
    kv.delete(&state_key).await?;

    let (client_id, client_secret) = client_pair(&env, &c)?;
    // 换 token（form 编码；GitHub 需 Accept: application/json 才回 JSON）
    let form = format!(
        "client_id={}&client_secret={}&code={}&redirect_uri={}&grant_type=authorization_code",
        urlencode(&client_id),
        urlencode(&client_secret),
        urlencode(&code),
        urlencode(&redirect_uri(&env, &provider))
    );
    let headers = worker::Headers::new();
    headers.set("content-type", "application/x-www-form-urlencoded")?;
    headers.set("accept", "application/json")?;
    let mut init = RequestInit::new();
    init.method = Method::Post;
    init.headers = headers;
    let bytes = worker::js_sys::Uint8Array::from(form.as_bytes());
    init.body = Some(bytes.into());
    let token_req = worker::Request::new_with_init(c.token_url, &init)?;
    let mut token_resp = Fetch::Request(token_req).send().await?;
    let token_json: serde_json::Value = token_resp.json().await?;
    let access_token = token_json
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Internal("oauth token exchange failed".into()))?
        .to_string();

    // 取用户信息
    let uheaders = worker::Headers::new();
    uheaders.set("authorization", &format!("Bearer {access_token}"))?;
    uheaders.set("accept", "application/json")?;
    uheaders.set("user-agent", "parallelspace-worker")?;
    let mut uinit = RequestInit::new();
    uinit.method = Method::Get;
    uinit.headers = uheaders;
    let user_req = worker::Request::new_with_init(c.userinfo_url, &uinit)?;
    let mut user_resp = Fetch::Request(user_req).send().await?;
    let user_json: serde_json::Value = user_resp.json().await?;

    let (subject, email) = if provider == "google" {
        let sub = user_json
            .get("sub")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Internal("google userinfo missing sub".into()))?
            .to_string();
        let verified = user_json
            .get("email_verified")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let email = user_json
            .get("email")
            .and_then(|v| v.as_str())
            .filter(|_| verified)
            .ok_or_else(|| AppError::Internal("google email not verified".into()))?
            .to_string();
        (sub, email)
    } else {
        let id = user_json
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| AppError::Internal("github user missing id".into()))?;
        let mut email = user_json
            .get("email")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if email.is_none() {
            // GitHub 邮箱可能不公开：走 /user/emails 取 primary 且已验证的
            let eheaders = worker::Headers::new();
            eheaders.set("authorization", &format!("Bearer {access_token}"))?;
            eheaders.set("accept", "application/json")?;
            eheaders.set("user-agent", "parallelspace-worker")?;
            let mut einit = RequestInit::new();
            einit.method = Method::Get;
            einit.headers = eheaders;
            let e_req = worker::Request::new_with_init("https://api.github.com/user/emails", &einit)?;
            let mut e_resp = Fetch::Request(e_req).send().await?;
            let list: serde_json::Value = e_resp.json().await?;
            email = list
                .as_array()
                .and_then(|arr| {
                    arr.iter().find(|e| {
                        e.get("primary").and_then(|v| v.as_bool()).unwrap_or(false)
                            && e.get("verified").and_then(|v| v.as_bool()).unwrap_or(false)
                    })
                })
                .and_then(|e| e.get("email").and_then(|v| v.as_str()).map(str::to_string));
        }
        let email = email
            .ok_or_else(|| AppError::Internal("github verified email not found".into()))?;
        (id.to_string(), email)
    };

    let email = email.to_lowercase();
    let account_id = resolve_or_create_account(&env, &provider, &subject, &email).await?;
    crate::governance::audit::record(
        &env,
        "auth.login",
        serde_json::json!({ "account_id": account_id, "provider": provider }),
    )
    .await?;

    // 浏览器流：带会话 Cookie 跳回应用首页
    let resp = respond_with_session(&env, &account_id, serde_json::json!({})).await?;
    let base = env
        .var("APP_BASE_URL")
        .map(|v| v.to_string())
        .unwrap_or_else(|_| "/".into());
    let cookie = resp
        .headers()
        .get("set-cookie")
        .ok()
        .flatten()
        .unwrap_or_default();
    let mut redirect = Response::redirect(base.parse().map_err(|_| {
        AppError::Internal("bad APP_BASE_URL".into())
    })?)?;
    redirect.headers_mut().set("set-cookie", &cookie)?;
    Ok(redirect)
}
