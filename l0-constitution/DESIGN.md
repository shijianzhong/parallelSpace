# parallelSpace L0 中立宪法 · 设计说明（草案 v0.1.0）

> 配套文件：`core.ttl`（纯本体草案）。本目录 **不进入当前构建**——现有 `ontology/` 仍是实现基线，本次不动一行代码。本草案供评审，通过后才进入下一轮迭代。

## 1. 为什么要重写 L0

当前本体把**人类制度快照**塞进了宪法层：`soc.ttl`（Organization 必须有章程、Position 科层职位、memberOf/holdsPosition 汇报线、owns 私有财产、Contract 契约、reputation/trustLevel）、`gov.ttl`（Constitution/Law/Charter、decisionMethod 把任命/选举/共识直接写成公理）、`econ.ttl`（Currency、市场挂单、雇佣、借贷）。

后果：agent 一开局就被关进一个"迷你人类社会"，只能复演人类已有的东西——这与"衍生出什么未知、可能更快更先进、不对标当前社会"直接冲突。**问题从来不是"加不加交通"，而是"L0 该有多中立"。**

## 2. L0 准入测试（唯一标准）

> **去掉它，agent 是否还能作为 agent 存在、互动是否还可能？**
> 是 → 物理/互动必然 → 可进 L0；否 → 人类制度模板 → 只能是 L2 涌现候选。

## 3. L0 中立内核（core.ttl，9 个原语）

| 原语 | 一句话理由 |
|---|---|
| `core:Agent` | 唯一主体，定义使然 |
| `core:Instant` / `core:tick` | 没有排序就没有动力学；不附历法/周期语义 |
| `core:Event`（append-only + `underOntologyVersion`） | 模拟的记忆；双版本路由的锚点 |
| `core:Action` ⊑ Event，**类型零枚举** | 能动性；没有 move/trade/speak 的清单 |
| `core:Entity` | 让"被发明的东西"可指称，但不预设它们是什么 |
| `core:Relation` + `from/to` | 一切社会结构的载体；拥有/信任/统治都是它的子类型候选 |
| `core:Observation` ⊑ Event | 没有观测就没有信息 |
| `core:Location` + `locatedAt`（可选） | 产品名是"平行空间"；但只到"位置可指称"为止 |
| `core:ChangeProposal` | 元宪法：允许社会改写自己（仅 L1/L2） |

## 4. 刻意排除（降级 L2，无一是必经）

组织/科层、契约、财产、货币/市场、雇佣、借贷、宪法/法律/章程、选举/任命/共识、声望/信任/技能、交通方式、语言、历法、**距离的度量方式**、任何行动类型枚举。

注意最后一条的深意：L0 甚至**不禁止**未来社会把 agent 变成关系的对象（如"拥有"一个 agent）——那是涌现社会自己的选择，只要不走 L0 修改。

## 5. 两个刻意的边界决定

1. **距离不进 L0**。"多远"依赖度量约定（欧氏/网络/耗时/关系亲疏……），约定即涌现。L0 只保证位置可指称。
2. **行动类型零枚举**。连 `Move` 都不预声明。若社会发展出移动，它自己会提案 `Move`（L2）。交通（飞机/高铁/汽车）同理——是 `TransportMode` 之类的 L2 取值，不是宪法。

## 6. 元宪法如何工作

```
agent 行为模式 ──LLM 概念化──> ChangeProposal(targetLayer=L2)
        │                          │
        │                     SHACL + OWL 2 RL 校验
        │                          │
        │                     L0 闸门（targetLayer=L0 一律拒，复用现有 touches_l0）
        │                          │
        │                     审批（机制不预设，治理实践涌现）
        │                          │
        └──> 发布为新本体版本 ──> 旧事件按旧版本解释 ──> 可回滚
```

LLM 的角色是**概念化**：从 agent 反复出现的行为模式里发现"它们似乎在持续做某种我们尚未命名的事"，提案为新概念（也许叫交通，也许叫我们完全想不到的名字），而不是拿人类制度填空。

## 7. 与现有脚手架的关系

- `ontology/`（soc/gov/econ/geo/time）保留为实现基线，本次不动。
- 采纳本草案后的迁移路径：`core.ttl` 成为新 L0；soc/gov/econ 降级为"参考域"移出宪法层（或仅作文档留存，不进运行时）；`ontology/version.rs` 的 L0 保护清单从 `Constitution/Agent/Event` 扩展为 core 全量原语；`api/admin.rs` 的文件名白名单（现锁死 soc/gov/econ/geo/time 五个）改为域注册表。
- 已就位的底层能力直接复用：双版本路由、`touches_l0` 闸门、R2 版本不覆盖发布、D1 `ontology_versions` + KV history、rollback。

## 8. 待你定的三件事

1. **`Location` 留不留 L0**——保留（产品名是"平行空间"，地图有渲染依托），还是连空间也交给涌现？
2. **参考域去向**——soc/gov/econ 是"降级为 L2 种子随社会演化激活"，还是"仅存文档、不进运行时"？
3. **下一步优先级**——(a) 本草案落地（重写 L0 + 迁移）；(b) 先补"域注册表 + pending 提案态"让治理员能手动发布新域；(c) LLM 概念化最小闭环。
