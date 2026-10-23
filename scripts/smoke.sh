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
REG2=$(post /api/auth/register "{\"email\":\"smoke2-$RUN_ID@example.com\",\"narrative\":\"x\"}")
AG2=$(echo "$REG2" | get agent_id)
chk "未付费分身激活 = 409" 409 "$(post_code "/api/agent/$AG2/activate" "$ACTP" -H "x-admin-token: $ADMIN_TOKEN")"
rm -rf "$TTL_DIR3"

echo
echo "结果：$pass 通过 / $fail 失败"
[ "$fail" -eq 0 ]
