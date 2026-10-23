#!/bin/bash
# 本地端到端冒烟：注册 → 下单 → 治理员确认入账 → 幂等重放 → 流水号查重 → 行动 → 查询/地图
#
# 用法：
#   Terminal A: npx wrangler dev --local --port 8787
#   Terminal B: bash scripts/smoke.sh [base_url]
#
# 需要 .dev.vars：
#   ADMIN_TOKEN=dev-admin-token
#   MANUAL_WECHAT_QR_URL=https://example.com/qr.png
#   ALLOW_DEV_OTP_ECHO=true   （仅本地：OTP 日志模式下回显验证码，供第 13 节）
#
# 两个脚本自身的历史坑（改脚本时别踩回去）：
# 1) macOS 自带 bash 3.2 对 `"$( cmd "arg" )"` 这种「双引号里再套双引号」解析有 bug，
#    会把参数拆错 → 请求变成 400。所以这里一律「先算成变量，再把变量传参」。
# 2) RDF 里有大量 `"`，直接插进 JSON 会破坏结构。凡带 RDF 的请求体一律交给
#    python 的 json.dumps 转义，不手写。
# 3) 每次运行用唯一渠道流水号，否则第二次跑会被上一轮订单占位拦下（拦截本身是对的）。
set -u
BASE="${1:-http://127.0.0.1:8787}"
ADMIN_TOKEN="${ADMIN_TOKEN:-dev-admin-token}"
RUN_ID="$(date +%s)-$$"
EMAIL="smoke-$RUN_ID@example.com"
TXN="WX-$RUN_ID"
pass=0; fail=0

chk() { # chk <name> <expected> <actual>
  if [ "$2" = "$3" ]; then
    echo "  PASS  $1"; pass=$((pass+1))
  else
    echo "  FAIL  $1  expected=[$2] actual=[$3]"; fail=$((fail+1))
  fi
}
# 所有请求都走这两个函数：payload 由调用方先算好，函数里不再出现嵌套引号
post()      { local p="$1" d="$2"; shift 2; curl -s --max-time 10 -X POST "$BASE$p" -H 'content-type: application/json' "$@" -d "$d"; }
post_code() { local p="$1" d="$2"; shift 2; curl -s -o /dev/null -w '%{http_code}' --max-time 10 -X POST "$BASE$p" -H 'content-type: application/json' "$@" -d "$d"; }
getcode()   { curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$1"; }
get()       { python3 -c "import sys,json;d=json.load(sys.stdin);print(d.get('$1',''))" 2>/dev/null || true; }

echo "== 0. health"
chk "GET /health = 200" 200 "$(getcode "$BASE/health")"

echo "== 1. 注册"
P="{\"email\":\"$EMAIL\",\"narrative\":\"我是个在港口长大的修船匠\"}"
REG=$(post /api/auth/register "$P")
ACC=$(echo "$REG" | get account_id)
AG=$(echo "$REG" | get agent_id)
chk "注册返回 account_id" yes "$([ -n "$ACC" ] && echo yes || echo no)"
chk "初始状态 created" created "$(echo "$REG" | get status)"

echo "== 2. 下单（服务端定价）"
P="{\"account_id\":\"$ACC\",\"package_id\":\"starter\"}"
ORD=$(post /api/payment/order "$P")
OID=$(echo "$ORD" | get order_id)
chk "starter = 1000 分" 1000 "$(echo "$ORD" | get amount_cents)"
chk "starter = 500 积分" 500 "$(echo "$ORD" | get credits)"
chk "返回收款码" https://example.com/qr.png "$(echo "$ORD" | get qr_code_url)"
P="{\"account_id\":\"$ACC\",\"package_id\":\"hack\"}"
chk "未知套餐 = 400" 400 "$(post_code /api/payment/order "$P")"
P='{"account_id":"nope","package_id":"starter"}'
chk "账号不存在 = 404" 404 "$(post_code /api/payment/order "$P")"
P="{\"account_id\":\"$ACC\",\"package_id\":\"starter\",\"amount_cents\":1}"
FORGED=$(post /api/payment/order "$P")
chk "客户端传 amount_cents 被忽略" 1000 "$(echo "$FORGED" | get amount_cents)"

echo "== 3. admin 令牌"
P="{\"order_id\":\"$OID\",\"channel_txn_id\":\"$TXN\"}"
chk "缺 x-admin-token = 401" 401 "$(post_code /api/payment/confirm "$P")"
chk "错 x-admin-token = 401" 401 "$(post_code /api/payment/confirm "$P" -H 'x-admin-token: wrong')"

echo "== 4. 金额二次核对"
P="{\"order_id\":\"$OID\",\"channel_txn_id\":\"$TXN\",\"expected_amount_cents\":1}"
chk "expected 金额不符 = 409" 409 "$(post_code /api/payment/confirm "$P" -H "x-admin-token: $ADMIN_TOKEN")"

echo "== 5. 治理员确认入账"
P="{\"order_id\":\"$OID\",\"channel_txn_id\":\"$TXN\",\"expected_amount_cents\":1000}"
CONF=$(post /api/payment/confirm "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "入账 500 积分" 500 "$(echo "$CONF" | get credits)"
chk "新余额 500" 500 "$(echo "$CONF" | get new_balance)"
chk "首次确认 already_confirmed=false" False "$(echo "$CONF" | get already_confirmed)"

echo "== 6. 幂等重放（同一订单再确认）"
P="{\"order_id\":\"$OID\",\"channel_txn_id\":\"$TXN\"}"
REP=$(post /api/payment/confirm "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "重放 already_confirmed=true" True "$(echo "$REP" | get already_confirmed)"
chk "重放余额不变 500" 500 "$(echo "$REP" | get new_balance)"

echo "== 7. 同一流水号绑到另一笔订单"
P="{\"account_id\":\"$ACC\",\"package_id\":\"standard\"}"
ORD2=$(post /api/payment/order "$P")
OID2=$(echo "$ORD2" | get order_id)
P="{\"order_id\":\"$OID2\",\"channel_txn_id\":\"$TXN\"}"
CHK7=$(post /api/payment/confirm "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "流水号复用 = 409" 409 "$(post_code /api/payment/confirm "$P" -H "x-admin-token: $ADMIN_TOKEN")"
chk "错误码可机读" CONFLICT "$(echo "$CHK7" | get error)"

echo "== 8. 行动：SHACL L2 守门"
# RDF 用真实文件承载（真换行、真引号），再由 python 读文件组 JSON——
# 任何「把 RDF 拼进 shell 字符串再转 JSON」的写法都会被引号/换行绕进去。
# 另外：被 soc:hasSkill / soc:locatedAt 引用的节点必须自带 rdf:type（SHACL 闭图判定）。
TTL_DIR=$(mktemp -d)
cat > "$TTL_DIR/bad.ttl" <<'TTL'
@prefix soc: <http://soc.example/ontology#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
<urn:agent:smoke> a soc:Person ;
    soc:status "flying" ;
    soc:displayName "n" ;
    soc:locatedAt <urn:l:1> ;
    soc:hasSkill <urn:s:1> ;
    soc:hasGoal <urn:g:1> ;
    soc:reputation "0.5"^^xsd:decimal .
<urn:l:1> a soc:Location .
<urn:s:1> a soc:Skill .
<urn:g:1> a soc:Goal .
TTL
sed 's/flying/active/' "$TTL_DIR/bad.ttl" > "$TTL_DIR/good.ttl"

mkaction() { python3 -c 'import json,os;rdf=open(os.environ["RDF_FILE"]).read();print(json.dumps({"agent_id":"urn:agent:smoke","action_type":os.environ["ACT"],"params":{"rdf":rdf}}))'; }

BAD=$(post /api/action "$(ACT=trade RDF_FILE="$TTL_DIR/bad.ttl" mkaction)")
chk "非法 status 被拒 accepted=false" False "$(echo "$BAD" | get accepted)"
chk "违规指向 status 路径" True "$(echo "$BAD" | python3 -c "import sys,json;print(any('#status' in v['path'] for v in json.load(sys.stdin)['violations']))")"

GOOD=$(post /api/action "$(ACT=work RDF_FILE="$TTL_DIR/good.ttl" mkaction)")
chk "合法负载被接受 accepted=true" True "$(echo "$GOOD" | get accepted)"
chk "返回 event_id" yes "$([ -n "$(echo "$GOOD" | get event_id)" ] && echo yes || echo no)"
rm -rf "$TTL_DIR"

echo "== 9. 查询与地图"
chk "GET /api/agent/:id = 200" 200 "$(getcode "$BASE/api/agent/$AG")"
chk "GET 不存在的 agent = 404" 404 "$(getcode "$BASE/api/agent/nope")"
MAP=$(curl -s --max-time 10 "$BASE/api/map/agents")
chk "地图返回 FeatureCollection" FeatureCollection "$(echo "$MAP" | get type)"

echo "== 10. 本体治理：域注册表 + 两段式发布（pending → approve）"
# 域注册表替代原硬编码文件名白名单；未审批的版本不写 KV、不生效。
# 域名每次运行唯一（tr-<run_id>），让脚本可重复跑而不被上一轮的注册挡下。
DOM="tr-$RUN_ID"
P="{\"domain\":\"$DOM\",\"layer\":\"L2\",\"description\":\"交通方式涌现候选（L2）\"}"
REG_D=$(post /api/admin/ontology/domains "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "注册新域 = seed" seed "$(echo "$REG_D" | get status)"
chk "重复注册 = 409" 409 "$(post_code /api/admin/ontology/domains "$P" -H "x-admin-token: $ADMIN_TOKEN")"
P="{\"domain\":\"core\",\"layer\":\"L2\"}"
chk "L0 保留域 core = 403" 403 "$(post_code /api/admin/ontology/domains "$P" -H "x-admin-token: $ADMIN_TOKEN")"
P="{\"domain\":\"whatever\",\"layer\":\"L0\"}"
chk "注册 L0 层 = 403" 403 "$(post_code /api/admin/ontology/domains "$P" -H "x-admin-token: $ADMIN_TOKEN")"
P="{\"domain\":\"bad\",\"layer\":\"L9\"}"
chk "非法层 = 400" 400 "$(post_code /api/admin/ontology/domains "$P" -H "x-admin-token: $ADMIN_TOKEN")"
P="{\"domain\":\"../soc\",\"layer\":\"L2\"}"
chk "路径穿越域名 = 400" 400 "$(post_code /api/admin/ontology/domains "$P" -H "x-admin-token: $ADMIN_TOKEN")"

# migration 0002 的五域种子（存在论纠偏：soc/gov/econ 降级 L2 seed）
DOMS=$(curl -s --max-time 10 "$BASE/api/admin/ontology/domains" -H "x-admin-token: $ADMIN_TOKEN")
chk "soc 域已注册为 seed" seed "$(echo "$DOMS" | python3 -c "import sys,json;print([d['status'] for d in json.load(sys.stdin) if d['domain']=='soc'][0])" 2>/dev/null)"

# 提案 payload 由 python 组（ttl 里有引号，不手写 JSON）
TTL_DIR2=$(mktemp -d)
cat > "$TTL_DIR2/$DOM.ttl" <<'TTL'
@prefix core: <http://parallelspace.org/ontology/core#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
core:TransportMode a owl:Class .
TTL
mkpub() { python3 -c 'import json,os;rdf=open(os.environ["RDF_FILE"]).read();print(json.dumps({"version":os.environ["VER"],"prior_version":os.environ["PRIOR"],"proposed_by":"admin","changed_concepts":[],"turtle_files":[{"name":os.environ["FNAME"],"content":rdf}]}))'; }

V1="smk1-$RUN_ID"
PUB=$(post /api/admin/ontology/publish "$(VER="$V1" PRIOR="1.0.0" FNAME="$DOM.ttl" RDF_FILE="$TTL_DIR2/$DOM.ttl" mkpub)" -H "x-admin-token: $ADMIN_TOKEN")
chk "提案落 pending（未生效）" pending "$(echo "$PUB" | get status)"
P='{"version":"x","prior_version":"1.0.0","changed_concepts":[],"turtle_files":[]}'
chk "缺 proposed_by = 400" 400 "$(post_code /api/admin/ontology/publish "$P" -H "x-admin-token: $ADMIN_TOKEN")"
chk "未注册域文件 = 400" 400 "$(post_code /api/admin/ontology/publish "$(VER="$V1" PRIOR="1.0.0" FNAME="nosuch.ttl" RDF_FILE="$TTL_DIR2/$DOM.ttl" mkpub)" -H "x-admin-token: $ADMIN_TOKEN")"
P="$(python3 -c 'import json;print(json.dumps({"version":"zz","prior_version":"1.0.0","proposed_by":"admin","changed_concepts":["http://soc.example/ontology#Agent"],"turtle_files":[]}))')"
chk "L0 概念变更 = 403" 403 "$(post_code /api/admin/ontology/publish "$P" -H "x-admin-token: $ADMIN_TOKEN")"
chk "重复提案同版本 = 409" 409 "$(post_code /api/admin/ontology/publish "$(VER="$V1" PRIOR="1.0.0" FNAME="$DOM.ttl" RDF_FILE="$TTL_DIR2/$DOM.ttl" mkpub)" -H "x-admin-token: $ADMIN_TOKEN")"

VERS=$(curl -s --max-time 10 "$BASE/api/admin/ontology/versions" -H "x-admin-token: $ADMIN_TOKEN")
chk "审批前 V1 = pending" pending "$(echo "$VERS" | python3 -c "import sys,json;print([v['status'] for v in json.load(sys.stdin) if v['version']=='$V1'][0])" 2>/dev/null)"

# 审批 → active；seed 域随激活
P="{\"approved_by\":\"governor\"}"
APR=$(post "/api/admin/ontology/versions/$V1/approve" "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "审批通过 = active" active "$(echo "$APR" | get status)"
VERS=$(curl -s --max-time 10 "$BASE/api/admin/ontology/versions" -H "x-admin-token: $ADMIN_TOKEN")
chk "审批后 V1 = active" active "$(echo "$VERS" | python3 -c "import sys,json;print([v['status'] for v in json.load(sys.stdin) if v['version']=='$V1'][0])" 2>/dev/null)"
DOMS=$(curl -s --max-time 10 "$BASE/api/admin/ontology/domains" -H "x-admin-token: $ADMIN_TOKEN")
chk "新域被激活" active "$(echo "$DOMS" | python3 -c "import sys,json;print([d['status'] for d in json.load(sys.stdin) if d['domain']=='$DOM'][0])" 2>/dev/null)"
chk "重复审批 = 409" 409 "$(post_code "/api/admin/ontology/versions/$V1/approve" "$P" -H "x-admin-token: $ADMIN_TOKEN")"
chk "审批不存在的版本 = 400" 400 "$(post_code "/api/admin/ontology/versions/nope/approve" "$P" -H "x-admin-token: $ADMIN_TOKEN")"

# 回滚：V2 审批后回滚到 V1，版本行状态互换
V2="smk2-$RUN_ID"
PUB2=$(post /api/admin/ontology/publish "$(VER="$V2" PRIOR="$V1" FNAME="$DOM.ttl" RDF_FILE="$TTL_DIR2/$DOM.ttl" mkpub)" -H "x-admin-token: $ADMIN_TOKEN")
chk "V2 提案落 pending" pending "$(echo "$PUB2" | get status)"
APR2=$(post "/api/admin/ontology/versions/$V2/approve" "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "V2 审批通过" active "$(echo "$APR2" | get status)"
P="{\"target_version\":\"$V1\",\"reason\":\"smoke rollback\"}"
RB=$(post /api/admin/ontology/rollback "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "回滚到 V1" "$V1" "$(echo "$RB" | get rolled_back_to)"
VERS=$(curl -s --max-time 10 "$BASE/api/admin/ontology/versions" -H "x-admin-token: $ADMIN_TOKEN")
chk "V2 = rolled_back" rolled_back "$(echo "$VERS" | python3 -c "import sys,json;print([v['status'] for v in json.load(sys.stdin) if v['version']=='$V2'][0])" 2>/dev/null)"
chk "V1 重新 active" active "$(echo "$VERS" | python3 -c "import sys,json;print([v['status'] for v in json.load(sys.stdin) if v['version']=='$V1'][0])" 2>/dev/null)"
P="{\"target_version\":\"never-approved\",\"reason\":\"x\"}"
chk "回滚到未审批版本 = 400" 400 "$(post_code /api/admin/ontology/rollback "$P" -H "x-admin-token: $ADMIN_TOKEN")"
rm -rf "$TTL_DIR2"

echo "== 11. 分身激活：pending → active（PersonCard 过 SHACL）"
# 支付首次确认已把分身推进到 pending（第 5 节确认入账之后）
ST=$(curl -s --max-time 10 "$BASE/api/agent/$AG")
chk "支付后状态 = pending" pending "$(echo "$ST" | get status)"
# PersonCard 文件（合法/非法各一，与第 8 节同一对卡：good 的 status 改自 bad 的 flying）
TTL_DIR3=$(mktemp -d)
cat > "$TTL_DIR3/bad.ttl" <<'TTL'
@prefix soc: <http://soc.example/ontology#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
<urn:agent:smoke> a soc:Person ;
    soc:status "flying" ;
    soc:displayName "n" ;
    soc:locatedAt <urn:l:1> ;
    soc:hasSkill <urn:s:1> ;
    soc:hasGoal <urn:g:1> ;
    soc:reputation "0.5"^^xsd:decimal .
<urn:l:1> a soc:Location .
<urn:s:1> a soc:Skill .
<urn:g:1> a soc:Goal .
TTL
sed 's/flying/active/' "$TTL_DIR3/bad.ttl" > "$TTL_DIR3/good.ttl"
mkact() { python3 -c 'import json,os;print(json.dumps({"person_card_rdf":open(os.environ["RDF_FILE"]).read()}))'; }
ACTP=$(RDF_FILE="$TTL_DIR3/good.ttl" mkact)
chk "激活缺 admin token = 401" 401 "$(post_code "/api/agent/$AG/activate" "$ACTP")"
ACTB=$(RDF_FILE="$TTL_DIR3/bad.ttl" mkact)
chk "坏 PersonCard 激活 = 422" 422 "$(post_code "/api/agent/$AG/activate" "$ACTB" -H "x-admin-token: $ADMIN_TOKEN")"
ACT=$(post "/api/agent/$AG/activate" "$ACTP" -H "x-admin-token: $ADMIN_TOKEN")
chk "合法卡激活 = active" active "$(echo "$ACT" | get status)"
ST=$(curl -s --max-time 10 "$BASE/api/agent/$AG")
chk "激活后查询状态 = active" active "$(echo "$ST" | get status)"
chk "重复激活 = 409" 409 "$(post_code "/api/agent/$AG/activate" "$ACTP" -H "x-admin-token: $ADMIN_TOKEN")"
# 未付费的新分身（created、无 paid 订单）激活 → 409
P="{\"email\":\"smoke2-$RUN_ID@example.com\",\"narrative\":\"x\"}"
REG2=$(post /api/auth/register "$P")
AG2=$(echo "$REG2" | get agent_id)
chk "未付费分身激活 = 409" 409 "$(post_code "/api/agent/$AG2/activate" "$ACTP" -H "x-admin-token: $ADMIN_TOKEN")"
rm -rf "$TTL_DIR3"


echo "== 12. 积分转让 / 收益 / 兑换（AGENTS §5.7）"
# ACC 当前余额 500（starter 已确认）。收款方用上面注册的第二账号 ACC2
ACC2=$(echo "$REG2" | get account_id)
P="{\"from_account_id\":\"$ACC\",\"to_account_id\":\"$ACC2\",\"amount\":200}"
chk "转让缺 token = 401" 401 "$(post_code /api/payment/transfer "$P")"
TR=$(post /api/payment/transfer "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "转让后出账余额 = 300" 300 "$(echo "$TR" | get fromBalance)"
chk "转让后入账余额 = 200" 200 "$(echo "$TR" | get toBalance)"
P="{\"from_account_id\":\"$ACC\",\"to_account_id\":\"$ACC2\",\"amount\":99999}"
chk "余额不足转让 = 409" 409 "$(post_code /api/payment/transfer "$P" -H "x-admin-token: $ADMIN_TOKEN")"
P="{\"from_account_id\":\"$ACC\",\"to_account_id\":\"$ACC\",\"amount\":10}"
chk "自转让 = 400" 400 "$(post_code /api/payment/transfer "$P" -H "x-admin-token: $ADMIN_TOKEN")"
P="{\"agent_id\":\"$AG\",\"credits\":150,\"source_ref\":\"smoke-gig-1\"}"
ER=$(post /api/admin/payment/agent-earnings "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "agent 收益入账后余额 = 450" 450 "$(echo "$ER" | get newBalance)"
P="{\"account_id\":\"$ACC\",\"credits\":100}"
RD=$(post /api/admin/payment/redeem "$P" -H "x-admin-token: $ADMIN_TOKEN")
RDID=$(echo "$RD" | get requestId)
chk "兑换金额 = 100 分（1 积分 = 1 分）" 100 "$(echo "$RD" | get amountCents)"
chk "兑换申请冻结后余额 = 350" 350 "$(echo "$RD" | get newBalance)"
P="{\"payout_ref\":\"wxpay-smoke-1\"}"
STL=$(post "/api/admin/payment/redeem/$RDID/settle" "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "兑换结算 = paid" paid "$(echo "$STL" | get status)"
P="{\"account_id\":\"$ACC\",\"credits\":50}"
RD2=$(post /api/admin/payment/redeem "$P" -H "x-admin-token: $ADMIN_TOKEN")
RDID2=$(echo "$RD2" | get requestId)
RJ=$(post "/api/admin/payment/redeem/$RDID2/reject" "{}" -H "x-admin-token: $ADMIN_TOKEN")
chk "兑换驳回释放后余额 = 350" 350 "$(echo "$RJ" | get newBalance)"


echo "== 13. 用户认证：邮箱验证码 + 会话 + 自助资金（G7）"
OTPEMAIL="otp-$RUN_ID@example.com"
P="{\"email\":\"$OTPEMAIL\"}"
START=$(post /api/auth/email/start "$P")
CODE=$(echo "$START" | get dev_code)
chk "OTP 已发（dev 回显）" yes "$([ -n "$CODE" ] && echo yes || echo no)"
P="{\"email\":\"$OTPEMAIL\",\"code\":\"000000\"}"
chk "错误验证码 = 401" 401 "$(post_code /api/auth/email/verify "$P")"
P="{\"email\":\"$OTPEMAIL\",\"code\":\"$CODE\"}"
VER=$(post /api/auth/email/verify "$P")
TOKEN2=$(echo "$VER" | get token)
ACC3=$(echo "$VER" | get account_id)
chk "验证成功下发会话" yes "$([ -n "$TOKEN2" ] && echo yes || echo no)"
P="{\"email\":\"$OTPEMAIL\",\"code\":\"$CODE\"}"
chk "验证码不可重放 = 400" 400 "$(post_code /api/auth/email/verify "$P")"
chk "无会话访问 /api/me = 401" 401 "$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$BASE/api/me")"
ME=$(curl -s --max-time 10 "$BASE/api/me" -H "Authorization: Bearer $TOKEN2")
chk "/api/me 邮箱一致" "$OTPEMAIL" "$(echo "$ME" | get email)"
P="{\"narrative\":\"otp smoke agent\"}"
NA=$(post /api/agent "$P" -H "Authorization: Bearer $TOKEN2")
AG3=$(echo "$NA" | get agent_id)
chk "会话建分身 = created" created "$(echo "$NA" | get status)"
P="{\"narrative\":\"again\"}"
chk "重复建分身 = 409" 409 "$(post_code /api/agent "$P" -H "Authorization: Bearer $TOKEN2")"
P="{\"agent_id\":\"$AG3\",\"credits\":120,\"source_ref\":\"smoke-g7\"}"
post /api/admin/payment/agent-earnings "$P" -H "x-admin-token: $ADMIN_TOKEN" >/dev/null
P="{\"credits\":40}"
RD3=$(post /api/me/redeem "$P" -H "Authorization: Bearer $TOKEN2")
chk "自助兑换后余额 = 80" 80 "$(echo "$RD3" | get newBalance)"
P="{\"to_account_id\":\"$ACC\",\"amount\":30}"
TR3=$(post /api/me/transfer "$P" -H "Authorization: Bearer $TOKEN2")
chk "自助转让后余额 = 50" 50 "$(echo "$TR3" | get fromBalance)"
ACTX=$(python3 -c "import json;print(json.dumps({'agent_id':'$AG','action_type':'work','params':{'rdf':'<urn:a> <urn:b> <urn:c> .'}}))")
chk "跨账号会话提交行动 = 403" 403 "$(post_code /api/action "$ACTX" -H "Authorization: Bearer $TOKEN2")"

echo "== 14. PersonCard 落库：注册带卡 → 激活优先用存量卡（G5）"
# 好/坏卡各一份（与第 11 节同一对：good 的 status 改自 bad 的 flying）
TTL_DIR5=$(mktemp -d)
cat > "$TTL_DIR5/bad.ttl" <<'TTL'
@prefix soc: <http://soc.example/ontology#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
<urn:agent:smoke> a soc:Person ;
    soc:status "flying" ;
    soc:displayName "n" ;
    soc:locatedAt <urn:l:1> ;
    soc:hasSkill <urn:s:1> ;
    soc:hasGoal <urn:g:1> ;
    soc:reputation "0.5"^^xsd:decimal .
<urn:l:1> a soc:Location .
<urn:s:1> a soc:Skill .
<urn:g:1> a soc:Goal .
TTL
sed 's/flying/active/' "$TTL_DIR5/bad.ttl" > "$TTL_DIR5/good.ttl"
# 新账号走 OTP 登录拿会话（dev 回显取码，与第 13 节同路）
OTPEMAIL5="otp5-$RUN_ID@example.com"
P="{\"email\":\"$OTPEMAIL5\"}"
START5=$(post /api/auth/email/start "$P")
CODE5=$(echo "$START5" | get dev_code)
P="{\"email\":\"$OTPEMAIL5\",\"code\":\"$CODE5\"}"
VER5=$(post /api/auth/email/verify "$P")
TOKEN5=$(echo "$VER5" | get token)
# 坏卡注册 = 422 且不建分身；随后无卡注册成功（证明 422 没留下分身）
mkagent() { python3 -c 'import json,os;d={"narrative":"g5 smoke agent"};f=os.environ.get("RDF_FILE");d["person_card_rdf"]=open(f).read() if f else None;d={k:v for k,v in d.items() if v is not None};print(json.dumps(d))'; }
BADP5=$(RDF_FILE="$TTL_DIR5/bad.ttl" mkagent)
chk "坏 PersonCard 注册 = 422" 422 "$(post_code /api/agent "$BADP5" -H "Authorization: Bearer $TOKEN5")"
NOP5=$(mkagent)
NA5=$(post /api/agent "$NOP5" -H "Authorization: Bearer $TOKEN5")
chk "坏卡 422 后无卡注册 = created" created "$(echo "$NA5" | get status)"
chk "无卡注册 person_card_stored = False" False "$(echo "$NA5" | get person_card_stored)"
# 带好卡注册（另一新账号）：落库 + 查询带出
OTPEMAIL6="otp6-$RUN_ID@example.com"
P="{\"email\":\"$OTPEMAIL6\"}"
START6=$(post /api/auth/email/start "$P")
CODE6=$(echo "$START6" | get dev_code)
P="{\"email\":\"$OTPEMAIL6\",\"code\":\"$CODE6\"}"
VER6=$(post /api/auth/email/verify "$P")
TOKEN6=$(echo "$VER6" | get token)
ACC6=$(echo "$VER6" | get account_id)
GOODP5=$(RDF_FILE="$TTL_DIR5/good.ttl" mkagent)
NA6=$(post /api/agent "$GOODP5" -H "Authorization: Bearer $TOKEN6")
AG6=$(echo "$NA6" | get agent_id)
chk "带卡注册 = created" created "$(echo "$NA6" | get status)"
chk "带卡注册 person_card_stored = True" True "$(echo "$NA6" | get person_card_stored)"
ST6=$(curl -s --max-time 10 "$BASE/api/agent/$AG6")
CARD6=$(echo "$ST6" | python3 -c "import sys,json;print('yes' if 'soc:Person' in (json.load(sys.stdin).get('person_card_rdf') or '') else 'no')" 2>/dev/null || echo no)
chk "查询带出已存 PersonCard" yes "$CARD6"
# 付费推进 pending（服务端定价 + admin 确认，流水号本轮唯一）
P="{\"account_id\":\"$ACC6\",\"package_id\":\"starter\"}"
ORD6=$(post /api/payment/order "$P")
OID6=$(echo "$ORD6" | get order_id)
P="{\"order_id\":\"$OID6\",\"channel_txn_id\":\"WX5-$RUN_ID\",\"expected_amount_cents\":1000}"
post /api/payment/confirm "$P" -H "x-admin-token: $ADMIN_TOKEN" >/dev/null
ST6=$(curl -s --max-time 10 "$BASE/api/agent/$AG6")
chk "付费后状态 = pending" pending "$(echo "$ST6" | get status)"
# 激活不带 card（空体）→ 用存量卡过 SHACL → active
chk "空体激活（用存量卡）= 200" 200 "$(post_code "/api/agent/$AG6/activate" '{}' -H "x-admin-token: $ADMIN_TOKEN")"
ST6=$(curl -s --max-time 10 "$BASE/api/agent/$AG6")
chk "存量卡激活后状态 = active" active "$(echo "$ST6" | get status)"
# 无卡分身（NA5 的 AG5）激活不带 card → 保持原报错语义 400
AG5=$(echo "$NA5" | get agent_id)
chk "无存量卡且不带卡激活 = 400" 400 "$(post_code "/api/agent/$AG5/activate" '{}' -H "x-admin-token: $ADMIN_TOKEN")"
rm -rf "$TTL_DIR5"

echo "== 15. G4 接线：沙箱 / 仲裁 / 回放 / 影响分析 / 离线代理闸门 / 组织创始人"
# 沙箱（G9 真沙箱）：proposal_id = 已发布的本体版本。好提案（第 10 节 V1，
# TransportMode 类定义）真装载 + 一致性 + SHACL + 快照仿真 → passed；
# 坏提案（实例声明为 owl:Nothing）与基线不一致 → failed 且不跑仿真（ticks_run=0）
P="{\"proposal_id\":\"$V1\",\"ticks\":3}"
SB=$(post /api/admin/sandbox/run "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "沙箱好提案 passed=true" True "$(echo "$SB" | get passed)"
chk "沙箱好提案 consistent=true" True "$(echo "$SB" | get consistent)"
chk "沙箱好提案 shacl_violations = 0" 0 "$(echo "$SB" | get shacl_violations)"
chk "沙箱 ticks_run = 3" 3 "$(echo "$SB" | get ticks_run)"
TTL_DIR8=$(mktemp -d)
cat > "$TTL_DIR8/soc.ttl" <<'TTL'
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<urn:smoke:bad> a owl:Nothing .
TTL
mkpub15() { python3 -c 'import json,os;rdf=open(os.environ["RDF_FILE"]).read();print(json.dumps({"version":os.environ["VER"],"prior_version":os.environ["PRIOR"],"proposed_by":"admin","changed_concepts":[],"turtle_files":[{"name":os.environ["FNAME"],"content":rdf}]}))'; }
VBAD="smkbad-$RUN_ID"
PUBB=$(post /api/admin/ontology/publish "$(VER="$VBAD" PRIOR="$V1" FNAME="soc.ttl" RDF_FILE="$TTL_DIR8/soc.ttl" mkpub15)" -H "x-admin-token: $ADMIN_TOKEN")
chk "坏提案落 pending" pending "$(echo "$PUBB" | get status)"
P="{\"proposal_id\":\"$VBAD\",\"ticks\":3}"
SBB=$(post /api/admin/sandbox/run "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "沙箱坏提案 passed=false" False "$(echo "$SBB" | get passed)"
chk "沙箱坏提案 consistent=false" False "$(echo "$SBB" | get consistent)"
chk "沙箱坏提案 ticks_run = 0" 0 "$(echo "$SBB" | get ticks_run)"
rm -rf "$TTL_DIR8"
P='{"proposal_id":"nope-ver","ticks":3}'
chk "沙箱不存在的版本 = 400" 400 "$(post_code /api/admin/sandbox/run "$P" -H "x-admin-token: $ADMIN_TOKEN")"
P="{\"proposal_id\":\"$V1\",\"ticks\":1001}"
chk "沙箱 ticks 超上限 = 400" 400 "$(post_code /api/admin/sandbox/run "$P" -H "x-admin-token: $ADMIN_TOKEN")"
P="{\"proposal_id\":\"$V1\",\"ticks\":3}"
chk "沙箱无 admin 令牌 = 401" 401 "$(post_code /api/admin/sandbox/run "$P")"
# 仲裁立案：双方为已存在的不同分身（AG 与 AG3）；原被告相同 = 400
P="{\"claimant_id\":\"$AG\",\"respondent_id\":\"$AG3\",\"claim\":\"smoke arbitration claim\"}"
ARB=$(post /api/admin/arbitration/cases "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "仲裁立案返回 case_id" yes "$([ -n "$(echo "$ARB" | get case_id)" ] && echo yes || echo no)"
P="{\"claimant_id\":\"$AG\",\"respondent_id\":\"$AG\",\"claim\":\"self case\"}"
chk "原被告相同立案 = 400" 400 "$(post_code /api/admin/arbitration/cases "$P" -H "x-admin-token: $ADMIN_TOKEN")"
# 回放：tick 0 空范围合法且报告版本不一致计数；倒置范围 = 400
RP=$(curl -s --max-time 10 "$BASE/api/admin/replay?from_tick=0&to_tick=0" -H "x-admin-token: $ADMIN_TOKEN")
chk "回放空范围 version_mismatches = 0" 0 "$(echo "$RP" | get version_mismatches)"
RPCODE=$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$BASE/api/admin/replay?from_tick=5&to_tick=4" -H "x-admin-token: $ADMIN_TOKEN")
chk "回放倒置范围 = 400" 400 "$RPCODE"
# 影响分析：第 10 节的 V1（域文件声明 core:TransportMode，当前无实例）
IMP=$(curl -s --max-time 10 "$BASE/api/admin/ontology/versions/$V1/impact" -H "x-admin-token: $ADMIN_TOKEN")
chk "影响分析版本回显" "$V1" "$(echo "$IMP" | get version)"
chk "影响分析概念含 TransportMode" yes "$(echo "$IMP" | python3 -c "import sys,json;print('yes' if any('TransportMode' in c['iri'] for c in json.load(sys.stdin)['concepts']) else 'no')" 2>/dev/null || echo no)"
chk "影响分析与基线一致 = True" True "$(echo "$IMP" | get consistent_with_baseline)"
chk "影响分析不存在的版本 = 400" 400 "$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$BASE/api/admin/ontology/versions/nope-ver/impact" -H "x-admin-token: $ADMIN_TOKEN")"
# 离线代理闸门：复用第 8 节同款 good 负载（SHACL 只看图形状，与 agent_id 无绑定）
TTL_DIR7=$(mktemp -d)
cat > "$TTL_DIR7/good.ttl" <<'TTL'
@prefix soc: <http://soc.example/ontology#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
<urn:agent:smoke> a soc:Person ;
    soc:status "active" ;
    soc:displayName "n" ;
    soc:locatedAt <urn:l:1> ;
    soc:hasSkill <urn:s:1> ;
    soc:hasGoal <urn:g:1> ;
    soc:reputation "0.5"^^xsd:decimal .
<urn:l:1> a soc:Location .
<urn:s:1> a soc:Skill .
<urn:g:1> a soc:Goal .
TTL
mkaction7() { python3 -c 'import json,os;rdf=open(os.environ["RDF_FILE"]).read();print(json.dumps({"agent_id":os.environ["AGID"],"action_type":os.environ["ACT"],"params":{"rdf":rdf}}))'; }
DEF=$(post /api/action "$(AGID="$AG3" ACT=trade RDF_FILE="$TTL_DIR7/good.ttl" mkaction7)")
chk "离线 trade 被 Defer" defer_to_owner "$(echo "$DEF" | get proxy_decision)"
chk "离线 trade accepted=false" False "$(echo "$DEF" | get accepted)"
DEN=$(post /api/action "$(AGID="$AG3" ACT=breach RDF_FILE="$TTL_DIR7/good.ttl" mkaction7)")
chk "离线 breach 被 Deny" deny "$(echo "$DEN" | get proxy_decision)"
WRK=$(post /api/action "$(AGID="$AG3" ACT=work RDF_FILE="$TTL_DIR7/good.ttl" mkaction7)")
chk "离线 work 放行 accepted=true" True "$(echo "$WRK" | get accepted)"
OWN=$(post /api/action "$(AGID="$AG3" ACT=trade RDF_FILE="$TTL_DIR7/good.ttl" mkaction7)" -H "Authorization: Bearer $TOKEN2")
chk "本人在线 trade 被接受" True "$(echo "$OWN" | get accepted)"
rm -rf "$TTL_DIR7"
# 组织创始人绑定：会话账号优先；无会话取入参；会话冒名 = 403
P="{\"name\":\"smoke-org-$RUN_ID\",\"charter_id\":\"charter-$RUN_ID\"}"
ORG1=$(post /api/org "$P" -H "Authorization: Bearer $TOKEN2")
chk "会话建组织创始人 = 会话账号" "$ACC3" "$(echo "$ORG1" | get founder_account_id)"
P="{\"name\":\"smoke-org2-$RUN_ID\",\"charter_id\":\"charter-$RUN_ID\",\"founder_account_id\":\"$ACC\"}"
ORG2=$(post /api/org "$P")
chk "无会话建组织创始人 = 入参" "$ACC" "$(echo "$ORG2" | get founder_account_id)"
P="{\"name\":\"smoke-org3-$RUN_ID\",\"charter_id\":\"charter-$RUN_ID\",\"founder_account_id\":\"$ACC\"}"
chk "会话冒名创始人 = 403" 403 "$(post_code /api/org "$P" -H "Authorization: Bearer $TOKEN2")"

echo "== 16. G8 位置与地图几何 / WebSocket 广播"
# 地点：治理通道创建（系统不预置地点数据）；id 每次运行唯一避免重跑冲突
LOC1="loc-a-$RUN_ID"
LOC2="loc-b-$RUN_ID"
P="{\"id\":\"$LOC1\",\"name\":\"港区\",\"lon\":116.40,\"lat\":39.90,\"polygon\":[[116.39,39.89],[116.41,39.89],[116.41,39.91],[116.39,39.91]]}"
chk "建地点缺 admin token = 401" 401 "$(post_code /api/admin/locations "$P")"
LC1=$(post /api/admin/locations "$P" -H "x-admin-token: $ADMIN_TOKEN")
chk "建地点返回 location_id" "$LOC1" "$(echo "$LC1" | get location_id)"
chk "带 polygon 的地点 geometry = Polygon" Polygon "$(echo "$LC1" | python3 -c "import sys,json;print(json.load(sys.stdin)['geometry']['type'])" 2>/dev/null || echo none)"
chk "重复 id 建地点 = 409" 409 "$(post_code /api/admin/locations "$P" -H "x-admin-token: $ADMIN_TOKEN")"
P='{"name":"bad","lon":1,"lat":999}'
chk "非法坐标建地点 = 400" 400 "$(post_code /api/admin/locations "$P" -H "x-admin-token: $ADMIN_TOKEN")"
P="{\"id\":\"$LOC2\",\"name\":\"山丘\",\"lon\":117.00,\"lat\":40.00}"
post /api/admin/locations "$P" -H "x-admin-token: $ADMIN_TOKEN" >/dev/null
LOCS=$(curl -s --max-time 10 "$BASE/api/map/locations")
chk "地点图层含新建地点" yes "$(echo "$LOCS" | python3 -c "import sys,json;d=json.load(sys.stdin);print('yes' if any(f['properties']['location_id']=='$LOC1' and f['geometry'] and f['geometry']['type']=='Polygon' for f in d['features']) else 'no')" 2>/dev/null || echo no)"
# 新分身：注册 → 下单 → 确认（推进 pending）→ 带落点激活
P="{\"email\":\"smoke16-$RUN_ID@example.com\",\"narrative\":\"x\"}"
REG16=$(post /api/auth/register "$P")
ACC16=$(echo "$REG16" | get account_id)
AG16=$(echo "$REG16" | get agent_id)
P="{\"account_id\":\"$ACC16\",\"package_id\":\"starter\"}"
ORD16=$(post /api/payment/order "$P")
OID16=$(echo "$ORD16" | get order_id)
P="{\"order_id\":\"$OID16\",\"channel_txn_id\":\"WX16-$RUN_ID\",\"expected_amount_cents\":1000}"
post /api/payment/confirm "$P" -H "x-admin-token: $ADMIN_TOKEN" >/dev/null
TTL_DIR16=$(mktemp -d)
cat > "$TTL_DIR16/good.ttl" <<'TTL'
@prefix soc: <http://soc.example/ontology#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
<urn:agent:smoke> a soc:Person ;
    soc:status "active" ;
    soc:displayName "n" ;
    soc:locatedAt <urn:l:1> ;
    soc:hasSkill <urn:s:1> ;
    soc:hasGoal <urn:g:1> ;
    soc:reputation "0.5"^^xsd:decimal .
<urn:l:1> a soc:Location .
<urn:s:1> a soc:Skill .
<urn:g:1> a soc:Goal .
TTL
mkact16() { python3 -c 'import json,os;print(json.dumps({"person_card_rdf":open(os.environ["RDF_FILE"]).read(),"location_id":os.environ["LOC"]}))'; }
ACT16BAD=$(RDF_FILE="$TTL_DIR16/good.ttl" LOC="nope-loc" mkact16)
chk "激活带未知地点 = 400" 400 "$(post_code "/api/agent/$AG16/activate" "$ACT16BAD" -H "x-admin-token: $ADMIN_TOKEN")"
ACT16=$(RDF_FILE="$TTL_DIR16/good.ttl" LOC="$LOC1" mkact16)
ACTR16=$(post "/api/agent/$AG16/activate" "$ACT16" -H "x-admin-token: $ADMIN_TOKEN")
chk "带落点激活 = active" active "$(echo "$ACTR16" | get status)"
chk "激活响应带 location_id" "$LOC1" "$(echo "$ACTR16" | get location_id)"
ST16=$(curl -s --max-time 10 "$BASE/api/agent/$AG16")
chk "分身 location_id 已落库" "$LOC1" "$(echo "$ST16" | get location_id)"
MAPA=$(curl -s --max-time 10 "$BASE/api/map/agents?status=active&bbox=116,39,118,41")
chk "地图分身 geometry = 落点坐标" "[116.4, 39.9]" "$(echo "$MAPA" | python3 -c "import sys,json;d=json.load(sys.stdin);f=[x for x in d['features'] if x['properties']['agent_id']=='$AG16'];import json as j;print(j.dumps(f[0]['geometry']['coordinates']) if f and f[0]['geometry'] else 'none')" 2>/dev/null || echo none)"
MAPB=$(curl -s --max-time 10 "$BASE/api/map/agents?status=active&bbox=0,0,10,10")
chk "bbox 外不含该分身（过滤已下推）" no "$(echo "$MAPB" | python3 -c "import sys,json;d=json.load(sys.stdin);print('yes' if any(x['properties']['agent_id']=='$AG16' for x in d['features']) else 'no')" 2>/dev/null || echo no)"
# Move：落点更新（params.location_id）；未知地点在落事件前拒绝
mkmove16() { python3 -c 'import json,os;print(json.dumps({"agent_id":os.environ["AGID"],"action_type":"move","params":{"rdf":open(os.environ["RDF_FILE"]).read(),"location_id":os.environ["LOC"]}}))'; }
MV16=$(post /api/action "$(AGID="$AG16" LOC="$LOC2" RDF_FILE="$TTL_DIR16/good.ttl" mkmove16)")
chk "Move 被接受" True "$(echo "$MV16" | get accepted)"
chk "Move 响应带新 location_id" "$LOC2" "$(echo "$MV16" | get location_id)"
ST16=$(curl -s --max-time 10 "$BASE/api/agent/$AG16")
chk "Move 后 location_id 已更新" "$LOC2" "$(echo "$ST16" | get location_id)"
chk "Move 到未知地点 = 400" 400 "$(post_code /api/action "$(AGID="$AG16" LOC="nope-loc" RDF_FILE="$TTL_DIR16/good.ttl" mkmove16)")"
# WebSocket：连 /api/ws → 收到 connected → 触发一次 Move → 收到该事件的广播
cat > "$TTL_DIR16/ws.mjs" <<'EOF'
import { readFileSync } from "node:fs";
const base = process.env.BASE_URL, AG = process.env.AGID;
const rdf = readFileSync(process.env.RDF_FILE, "utf8");
const ws = new WebSocket(base.replace("http://", "ws://") + "/api/ws");
const timer = setTimeout(() => { console.log("WS_TIMEOUT"); process.exit(1); }, 9000);
ws.onmessage = async (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.kind === "connected") {
    const r = await fetch(base + "/api/action", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ agent_id: AG, action_type: "move", params: { rdf, location_id: process.env.LOC } }) });
    const j = await r.json();
    if (!j.accepted) { console.log("WS_MOVE_REJECTED"); clearTimeout(timer); process.exit(1); }
  }
  if (msg.kind === "event" && msg.event && msg.event.payload && msg.event.payload.agent_id === AG && msg.event.payload.action_type === "move") {
    console.log("WS_EVENT_OK"); clearTimeout(timer); process.exit(0);
  }
};
ws.onerror = () => { console.log("WS_ERROR"); clearTimeout(timer); process.exit(1); };
EOF
WSOUT=$(BASE_URL="$BASE" AGID="$AG16" RDF_FILE="$TTL_DIR16/good.ttl" LOC="$LOC1" node "$TTL_DIR16/ws.mjs" 2>/dev/null || true)
chk "WS 订阅收到 Move 事件广播" WS_EVENT_OK "$WSOUT"
rm -rf "$TTL_DIR16"

echo
echo "结果：$pass 通过 / $fail 失败"
[ "$fail" -eq 0 ]
