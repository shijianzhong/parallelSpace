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

echo
echo "结果：$pass 通过 / $fail 失败"
[ "$fail" -eq 0 ]
