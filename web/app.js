"use strict";
/* ParallelSpace 前端入口（试运行 MVP）：零构建原生 JS。
   数据全部来自本服务 API，会话靠 ps_session Cookie（fetch 带 credentials）。
   世界层不对访客开放：非登录页先查 /api/me，401 即跳 /login。 */

const $ = (s) => document.querySelector(s);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

async function api(path, opts = {}) {
  const res = await fetch(path, { credentials: "same-origin", ...opts });
  let data = null;
  try { data = await res.json(); } catch { /* 非 JSON 响应留空 */ }
  return { status: res.status, data };
}
function apiJson(path, body, headers = {}) {
  return api(path, { method: "POST", headers: { "content-type": "application/json", ...headers }, body: JSON.stringify(body) });
}
function errText(r, fallback) {
  if (r.data && (r.data.message || r.data.error)) return `${r.data.message || ""} ${r.data.error ? "(" + r.data.error + ")" : ""}`.trim();
  return fallback || `请求失败（HTTP ${r.status}）`;
}
function setMsg(el, text, ok) {
  el.textContent = text || "";
  el.className = "msg " + (text ? (ok ? "ok" : "err") : "");
}

const VIEWS = { "/": "home", "/home": "home", "/login": "login", "/create": "create", "/timeline": "timeline", "/funds": "funds", "/map": "map" };
const view = VIEWS[location.pathname] || "home";
let me = null;          // /api/me 结果（登录后）
let locationsCache = []; // /api/map/locations 的 features

async function loadLocations() {
  const r = await api("/api/map/locations?limit=1000");
  locationsCache = (r.data && r.data.features) || [];
  return locationsCache;
}
function locationName(id) {
  if (!id) return null;
  const f = locationsCache.find((x) => x.properties && x.properties.location_id === id);
  return f ? f.properties.name : id;
}

/* ---------------- 首页：分身卡 ---------------- */
async function initHome() {
  const body = $("#home-body");
  if (!me.agent) {
    body.innerHTML = `<div class="card"><p>你还没有分身。</p><p><a class="btn" href="/create">去创建分身</a></p></div>`;
    return;
  }
  const a = me.agent;
  await loadLocations();
  let locId = null, locName = null;
  const detail = await api(`/api/agent/${encodeURIComponent(a.id)}`);
  if (detail.status === 200 && detail.data) {
    locId = detail.data.location_id;
    locName = locationName(locId);
  }
  const hs = a.hot_state;
  let html = `<div class="card">
    <p><span class="badge ${esc(a.status)}">${esc(a.status)}</span> <code>${esc(a.id)}</code></p>
    <div class="kv">
      <div><b>${me.credits}</b><span>积分余额</span></div>
      <div><b>${a.person_card_rdf ? "已存" : "未存"}</b><span>PersonCard</span></div>
      <div><b>${locName ? esc(locName) : "未落点"}</b><span>当前位置</span></div>
      ${hs ? `<div><b>${esc(hs.energy)}</b><span>能量</span></div>
      <div><b>${esc(hs.food)}</b><span>食物</span></div>
      <div><b>${esc(hs.money)}</b><span>社会内资金</span></div>
      <div><b>${esc(hs.reputation)}</b><span>声望</span></div>` : ""}
    </div>
    <p class="muted small">账号 <code>${esc(me.account_id)}</code> · ${esc(me.email)}</p>`;
  if (a.status === "created") {
    html += `<p class="muted">分身已创建。按当前流程，支付由运营方人工确认，确认后状态进入「待激活（pending）」，再由治理员激活。</p>`;
  }
  if (a.status === "pending") {
    html += `<div class="card">
      <h2>激活分身</h2>
      <p class="muted small">激活是治理动作，需要治理员令牌；默认使用注册时已存的 PersonCard，也可在此临时覆盖。出生落点可选（地点由治理通道创建）。</p>
      <form id="activate-form">
        <label>治理员令牌（x-admin-token）
          <input id="act-token" type="password" autocomplete="off">
        </label>
        <label>PersonCard RDF 覆盖（可空 = 用已存卡）
          <textarea id="act-card" rows="6" class="mono"></textarea>
        </label>
        <label>出生落点（可空）
          <select id="act-loc"><option value="">不设置</option></select>
        </label>
        <button type="submit">激活</button>
        <span id="act-msg" class="msg"></span>
        <ul id="act-violations" class="violations"></ul>
      </form>
    </div>`;
  }
  html += `<p><a href="/timeline">看生活时间线</a> · <a href="/map">去地图</a> · <a href="/funds">资金与流水</a></p></div>`;
  body.innerHTML = html;

  if (a.status === "pending") {
    const sel = $("#act-loc");
    for (const f of locationsCache) {
      const o = document.createElement("option");
      o.value = f.properties.location_id;
      o.textContent = f.properties.name;
      sel.appendChild(o);
    }
    $("#activate-form").addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const msg = $("#act-msg"), vio = $("#act-violations");
      vio.innerHTML = ""; setMsg(msg, "激活中…", true);
      const payload = {};
      const card = $("#act-card").value.trim();
      if (card) payload.person_card_rdf = card;
      if (sel.value) payload.location_id = sel.value;
      const token = $("#act-token").value.trim();
      const r = await apiJson(`/api/agent/${encodeURIComponent(a.id)}/activate`, payload, token ? { "x-admin-token": token } : {});
      if (r.status === 200 && r.data && r.data.status === "active") {
        setMsg(msg, "激活成功，刷新中…", true);
        setTimeout(() => location.reload(), 600);
      } else if (r.status === 422 && r.data && Array.isArray(r.data.violations)) {
        setMsg(msg, "PersonCard 未通过校验：", false);
        vio.innerHTML = r.data.violations.map((v) => `<li>${esc(v.path)}：${esc(v.message)}</li>`).join("");
      } else {
        setMsg(msg, errText(r, "激活失败"), false);
      }
    });
  }
}

/* ---------------- 创建分身 ---------------- */
const EXAMPLE_CARD = `@prefix soc: <http://soc.example/ontology#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
<urn:agent:me> a soc:Person ;
    soc:status "active" ;
    soc:displayName "示例分身" ;
    soc:locatedAt <urn:loc:home> ;
    soc:hasSkill <urn:skill:craft> ;
    soc:hasGoal <urn:goal:explore> ;
    soc:reputation "0.5"^^xsd:decimal .
<urn:loc:home> a soc:Location .
<urn:skill:craft> a soc:Skill .
<urn:goal:explore> a soc:Goal .
`;
function initCreate() {
  if (me.agent) {
    $("#create-body").innerHTML = `<div class="card"><p>你已经有分身了（一账号一个）：<code>${esc(me.agent.id)}</code>，状态 ${esc(me.agent.status)}。</p><p><a class="btn" href="/">回我的分身</a></p></div>`;
    return;
  }
  $("#card-example-btn").addEventListener("click", () => { $("#create-card").value = EXAMPLE_CARD; });
  $("#create-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const msg = $("#create-msg"), vio = $("#create-violations");
    vio.innerHTML = ""; setMsg(msg, "提交中…", true);
    const payload = { narrative: $("#create-narrative").value };
    const card = $("#create-card").value.trim();
    if (card) payload.person_card_rdf = card;
    const r = await apiJson("/api/agent", payload);
    if (r.status === 200) {
      setMsg(msg, "创建成功，跳转首页…", true);
      setTimeout(() => { location.href = "/"; }, 600);
    } else if (r.status === 422 && r.data && Array.isArray(r.data.violations)) {
      setMsg(msg, "PersonCard 未通过 SHACL 校验，分身未创建：", false);
      vio.innerHTML = r.data.violations.map((v) => `<li>${esc(v.path)}：${esc(v.message)}</li>`).join("");
    } else if (r.status === 401) {
      location.replace("/login");
    } else {
      setMsg(msg, errText(r, "创建失败"), false);
    }
  });
}

/* ---------------- 时间线 ---------------- */
const ACTION_LABEL = { move: "移动", work: "工作", trade: "交易", chat: "交谈", learn: "学习", ally: "结盟", employ: "雇佣", vote: "投票", legislate: "立法", breach: "违约类行动" };
function eventTitle(item) {
  const t = item.type || "";
  if (t.endsWith("#Action") || t.endsWith("/Action")) {
    const at = item.payload && item.payload.action_type;
    return `行动 · ${ACTION_LABEL[at] || at || "未知"}`;
  }
  if (t.includes("AgentActivated")) return "分身被激活";
  return t.split("#").pop().split("/").pop() || t;
}
function renderTimeline(items) {
  if (!items.length) return `<div class="card"><p class="muted">暂无记录。</p></div>`;
  return items.map((item) => {
    if (item.kind === "audit") {
      const f = item.fields || {};
      const deferred = item.action === "action.proxy_deferred";
      return `<div class="tl-item audit">
        <div class="t">${deferred ? "行动被暂缓（离线代理）" : "行动被拒绝（离线代理）"}</div>
        <div class="meta">${esc(item.at)} · 审计记录</div>
        <p>${deferred
          ? "你离线期间，分身尝试了一个不可逆行动，已被离线代理闸门拦下，<b>没有执行</b>，等待你本人决定（决策收件箱尚未上线，先在此呈现）。"
          : "你离线期间，分身尝试了一个代理无权同意的行动类别，已被离线代理闸门<b>直接拒绝</b>。"}</p>
        <p class="small muted">行动类型：${esc(ACTION_LABEL[f.action_type] || f.action_type || "")}（${esc(f.action_type || "")}）· 裁决：${esc(f.proxy_decision || "")}</p>
      </div>`;
    }
    const p = item.payload;
    return `<div class="tl-item">
      <div class="t">${esc(eventTitle(item))}</div>
      <div class="meta">tick ${esc(item.tick)} · ${esc(item.at)} · 本体版本 ${esc(item.ontology_version || "")}</div>
      ${p ? `<details><summary>详情</summary><pre>${esc(JSON.stringify(p, null, 2))}</pre></details>` : `<p class="small muted">（事件体未取到，仅索引信息）</p>`}
    </div>`;
  }).join("");
}
async function initTimeline() {
  const body = $("#timeline-body");
  const load = async () => {
    body.innerHTML = `<p class="muted">加载中…</p>`;
    const r = await api("/api/me/timeline?limit=100");
    if (r.status === 401) { location.replace("/login"); return; }
    if (r.status !== 200 || !r.data) { body.innerHTML = `<p class="msg err">${esc(errText(r, "时间线加载失败"))}</p>`; return; }
    if (!r.data.agent_id) { body.innerHTML = `<div class="card"><p>你还没有分身。</p><p><a class="btn" href="/create">去创建分身</a></p></div>`; return; }
    body.innerHTML = renderTimeline(r.data.items || []);
  };
  $("#timeline-refresh").addEventListener("click", load);
  await load();
}

/* ---------------- 资金 ---------------- */
const TXN_LABEL = { purchase: "购买入账", transfer_out: "转出", transfer_in: "转入", agent_earnings: "分身收益入账", redeem_hold: "兑换冻结", redeem_release: "兑换驳回释放" };
async function initFunds() {
  const body = $("#funds-body"), txn = $("#txn-body");
  body.innerHTML = `<div class="card"><div class="kv">
    <div><b>${me.credits}</b><span>积分余额</span></div>
    <div><b><code>${esc(me.account_id)}</code></b><span>我的账号 ID（转出方填这个找我）</span></div>
  </div></div>`;
  const loadTxn = async () => {
    const r = await api("/api/me/transactions?limit=100");
    if (r.status !== 200 || !r.data) { txn.innerHTML = `<p class="msg err">${esc(errText(r, "流水加载失败"))}</p>`; return; }
    const rows = r.data.transactions || [];
    if (!rows.length) { txn.innerHTML = `<p class="muted">暂无流水。</p>`; return; }
    txn.innerHTML = `<table><thead><tr><th>时间</th><th>类型</th><th>变动</th><th>关联</th></tr></thead><tbody>${
      rows.map((t) => `<tr><td>${esc(t.created_at)}</td><td>${esc(TXN_LABEL[t.type] || t.type)}</td><td class="${t.delta >= 0 ? "delta-pos" : "delta-neg"}">${t.delta >= 0 ? "+" : ""}${esc(t.delta)}</td><td><code>${esc(t.order_id || t.event_id || "")}</code></td></tr>`).join("")
    }</tbody></table>`;
  };
  await loadTxn();
  $("#transfer-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const msg = $("#transfer-msg"); setMsg(msg, "提交中…", true);
    const r = await apiJson("/api/me/transfer", { to_account_id: $("#transfer-to").value.trim(), amount: Number($("#transfer-amount").value) });
    if (r.status === 200 && r.data) {
      setMsg(msg, `转让成功，我的新余额：${r.data.fromBalance}`, true);
      await loadTxn();
    } else setMsg(msg, errText(r, "转让失败"), false);
  });
  $("#redeem-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const msg = $("#redeem-msg"); setMsg(msg, "提交中…", true);
    const r = await apiJson("/api/me/redeem", { credits: Number($("#redeem-credits").value) });
    if (r.status === 200 && r.data) {
      setMsg(msg, `申请已提交：单号 ${r.data.requestId}，状态 ${r.data.status}，折合 ${r.data.amountCents} 分，冻结后余额 ${r.data.newBalance}。等待治理员审批。`, true);
      await loadTxn();
    } else setMsg(msg, errText(r, "兑换申请失败"), false);
  });
}

/* ---------------- 地图 ---------------- */
function renderMap(locFeatures, agentFeatures) {
  const pts = [];
  const polys = [];
  for (const f of locFeatures) {
    const g = f.geometry; if (!g) continue;
    if (g.type === "Point") pts.push({ lon: g.coordinates[0], lat: g.coordinates[1], name: f.properties.name, kind: "loc" });
    if (g.type === "Polygon") { polys.push({ ring: g.coordinates[0], name: f.properties.name }); for (const c of g.coordinates[0]) pts.push({ lon: c[0], lat: c[1] }); }
  }
  const agents = [];
  const noPos = [];
  for (const f of agentFeatures) {
    const g = f.geometry;
    if (g && g.type === "Point") { agents.push({ lon: g.coordinates[0], lat: g.coordinates[1], id: f.properties.agent_id, status: f.properties.status, loc: f.properties.location_name }); pts.push({ lon: g.coordinates[0], lat: g.coordinates[1] }); }
    else noPos.push(f.properties);
  }
  if (!locFeatures.length && !agentFeatures.length) {
    return `<div class="card"><p class="muted">暂无地点与分身位置数据（系统不预置地点）。</p></div>`;
  }
  if (!pts.length) {
    return `<div class="card"><p class="muted">已有 ${locFeatures.length} 个地点、${agentFeatures.length} 个分身，但都暂无坐标。</p></div>`;
  }
  let minLon = Math.min(...pts.map((p) => p.lon)), maxLon = Math.max(...pts.map((p) => p.lon));
  let minLat = Math.min(...pts.map((p) => p.lat)), maxLat = Math.max(...pts.map((p) => p.lat));
  const padX = Math.max((maxLon - minLon) * 0.15, 0.01), padY = Math.max((maxLat - minLat) * 0.15, 0.01);
  minLon -= padX; maxLon += padX; minLat -= padY; maxLat += padY;
  const span = Math.max(maxLon - minLon, maxLat - minLat);
  const r = span / 90, fs = span / 28;
  const X = (lon) => lon, Y = (lat) => -lat;
  let svg = `<svg id="map-svg" viewBox="${minLon} ${-maxLat} ${maxLon - minLon} ${maxLat - minLat}" role="img" aria-label="世界地图">`;
  for (const p of polys) {
    svg += `<polygon points="${p.ring.map((c) => `${c[0]},${-c[1]}`).join(" ")}" fill="#dbeafe" stroke="#2563eb" stroke-width="${span / 400}"><title>${esc(p.name)}</title></polygon>`;
  }
  for (const p of pts.filter((x) => x.kind === "loc")) {
    svg += `<circle cx="${X(p.lon)}" cy="${Y(p.lat)}" r="${r}" fill="#2563eb"><title>${esc(p.name)}</title></circle>`;
    svg += `<text x="${X(p.lon) + r * 1.4}" y="${Y(p.lat)}" font-size="${fs}" fill="#1c2430">${esc(p.name)}</text>`;
  }
  for (const a of agents) {
    const mine = me.agent && a.id === me.agent.id;
    svg += `<circle cx="${X(a.lon)}" cy="${Y(a.lat)}" r="${mine ? r * 1.5 : r}" fill="${mine ? "#ea580c" : "#64748b"}" stroke="#fff" stroke-width="${span / 500}"><title>${esc(a.id)}（${esc(a.status)}）${a.loc ? " @ " + esc(a.loc) : ""}</title></circle>`;
    if (mine) svg += `<text x="${X(a.lon) + r * 2}" y="${Y(a.lat)}" font-size="${fs}" fill="#ea580c">我的分身</text>`;
  }
  svg += `</svg>`;
  if (noPos.length) {
    svg += `<p class="muted small">暂无位置的分身：${noPos.map((p) => `<code>${esc(p.agent_id)}</code>（${esc(p.status)}）`).join("、")}</p>`;
  }
  return svg;
}
async function initMap() {
  const body = $("#map-body"), feed = $("#map-feed"), status = $("#ws-status");
  const refresh = async () => {
    const [locs, ags] = await Promise.all([api("/api/map/locations?limit=1000"), api("/api/map/agents?limit=1000")]);
    locationsCache = (locs.data && locs.data.features) || [];
    body.innerHTML = renderMap(locationsCache, (ags.data && ags.data.features) || []);
  };
  await refresh();
  let debounce = null;
  const connect = () => {
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const ws = new WebSocket(`${proto}//${location.host}/api/ws`);
    ws.onopen = () => { status.textContent = "实时事件：已连接"; };
    ws.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data);
        if (msg.kind === "event" && msg.event) {
          const e = msg.event;
          const line = document.createElement("div");
          const label = e.type && e.type.includes("AgentActivated") ? "分身被激活" : (e.payload && e.payload.action_type ? `行动 · ${ACTION_LABEL[e.payload.action_type] || e.payload.action_type}` : (e.type || "事件"));
          line.textContent = `${new Date().toLocaleTimeString()} 新事件：${label}${e.actor_id ? "（" + e.actor_id.slice(0, 8) + "…）" : ""}`;
          feed.prepend(line);
          clearTimeout(debounce);
          debounce = setTimeout(refresh, 800);
        }
      } catch { /* 非 JSON 消息忽略 */ }
    };
    ws.onclose = () => { status.textContent = "实时事件：已断开，3 秒后重连"; setTimeout(connect, 3000); };
    ws.onerror = () => { try { ws.close(); } catch { /* ignore */ } };
  };
  connect();
}

/* ---------------- 登录 ---------------- */
function initLogin() {
  let email = "";
  $("#otp-start-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const msg = $("#otp-start-msg"); setMsg(msg, "发送中…", true);
    email = $("#otp-email").value.trim();
    const r = await apiJson("/api/auth/email/start", { email });
    if (r.status === 200) {
      let text = "验证码已发送，请查收邮箱（10 分钟内有效）。";
      if (r.data && r.data.dev_code) text += ` 本地开发模式验证码：${r.data.dev_code}`;
      setMsg(msg, text, true);
    } else setMsg(msg, errText(r, "发送失败"), false);
  });
  $("#otp-verify-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const msg = $("#otp-verify-msg"); setMsg(msg, "验证中…", true);
    const useEmail = email || $("#otp-email").value.trim();
    const r = await apiJson("/api/auth/email/verify", { email: useEmail, code: $("#otp-code").value.trim() });
    if (r.status === 200) { setMsg(msg, "登录成功，跳转中…", true); location.href = "/"; }
    else setMsg(msg, errText(r, "验证失败"), false);
  });
}

/* ---------------- 启动 ---------------- */
(async function boot() {
  document.querySelectorAll("section[data-view]").forEach((s) => { s.hidden = s.dataset.view !== view; });
  document.querySelectorAll("nav#tabs a").forEach((a) => { if (a.dataset.nav === view) a.classList.add("on"); });
  if (view === "login") {
    const probe = await api("/api/me");
    if (probe.status === 200) { location.replace("/"); return; }
    $("#boot-msg").hidden = true;
    initLogin();
    return;
  }
  const r = await api("/api/me");
  if (r.status === 401) { location.replace("/login"); return; }
  if (r.status !== 200 || !r.data) { $("#boot-msg").textContent = "加载失败，请刷新重试。"; return; }
  me = r.data;
  $("#tabs").hidden = false;
  $("#whoami").hidden = false;
  $("#whoami-email").textContent = me.email;
  $("#logout-btn").addEventListener("click", async () => {
    await api("/api/auth/logout", { method: "POST" });
    location.replace("/login");
  });
  $("#boot-msg").hidden = true;
  if (view === "home") await initHome();
  else if (view === "create") initCreate();
  else if (view === "timeline") await initTimeline();
  else if (view === "funds") await initFunds();
  else if (view === "map") await initMap();
})();
