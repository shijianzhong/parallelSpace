//! L2 SHACL 校验引擎。
//!
//! 定位（AGENTS.md §5 分层校验）：
//! - OWL 管语义推理（ontology/reason.rs 的 reasonable OWL 2 RL + L3 open-ontologies）；
//! - SHACL 管数据形状与业务规则，就是本模块；
//! - L2 必须在 < 50ms 内拒绝明显非法的负载，因此这里是一个**有界子集**实现。
//!
//! 支持：sh:targetClass / sh:property / sh:path / sh:minCount / sh:maxCount /
//! sh:datatype / sh:in / sh:minLength / sh:maxLength / sh:minInclusive /
//! sh:maxInclusive / sh:class / sh:message；rdfs:subClassOf 闭包（sh:class 与
//! targetClass 均按 SHACL「SHACL instance」语义走子类展开）。
//! 不支持（留给 L3）：sh:node / sh:or / sh:xone / sh:not / SPARQL 约束 /
//! sh:targetNode / RDF-star。
//!
//! 形状文件是单一事实来源：`ontology/shacl/*.ttl` 经 include_str! 内嵌，
//! 改约束改 .ttl，不要改本文件。

use std::collections::HashSet;
use std::io::BufReader;
use std::sync::OnceLock;

use rio_api::model::{Literal, Subject, Term, Triple};
use rio_api::parser::TriplesParser;
use rio_turtle::{TurtleError, TurtleParser};

use crate::common::error::{AppError, AppResult};
use crate::common::types::{ValidationResult, Violation};
use crate::governance::validator::ActionType;

// ---- SHACL / RDF 词表（iri.rs 管业务命名空间，这里管 W3C 词表）----
const SH_NODE_SHAPE: &str = "http://www.w3.org/ns/shacl#NodeShape";
const SH_TARGET_CLASS: &str = "http://www.w3.org/ns/shacl#targetClass";
const SH_PROPERTY: &str = "http://www.w3.org/ns/shacl#property";
const SH_PATH: &str = "http://www.w3.org/ns/shacl#path";
const SH_MIN_COUNT: &str = "http://www.w3.org/ns/shacl#minCount";
const SH_MAX_COUNT: &str = "http://www.w3.org/ns/shacl#maxCount";
const SH_DATATYPE: &str = "http://www.w3.org/ns/shacl#datatype";
const SH_IN: &str = "http://www.w3.org/ns/shacl#in";
const SH_MIN_LENGTH: &str = "http://www.w3.org/ns/shacl#minLength";
const SH_MAX_LENGTH: &str = "http://www.w3.org/ns/shacl#maxLength";
const SH_MIN_INCLUSIVE: &str = "http://www.w3.org/ns/shacl#minInclusive";
const SH_MAX_INCLUSIVE: &str = "http://www.w3.org/ns/shacl#maxInclusive";
const SH_CLASS: &str = "http://www.w3.org/ns/shacl#class";
const SH_MESSAGE: &str = "http://www.w3.org/ns/shacl#message";
// 注：SHACL 集合用 rdf:first / rdf:rest（sh:first / sh:rest 已被规范移除）
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// 内嵌形状文件：改约束请改 ontology/shacl/*.ttl。
pub const SHAPE_FILES: &[(&str, &str)] = &[
    (
        "person-card",
        include_str!("../../ontology/shacl/person-card.ttl"),
    ),
    ("charter", include_str!("../../ontology/shacl/charter.ttl")),
    (
        "transaction",
        include_str!("../../ontology/shacl/transaction.ttl"),
    ),
];

// ===================== RDF 图模型 =====================

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Node {
    Iri(String),
    Blank(String),
}

impl Node {
    fn key(&self) -> String {
        match self {
            Node::Iri(i) => format!("<{i}>"),
            Node::Blank(b) => format!("_:{b}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Iri(String),
    Blank(String),
    Literal {
        value: String,
        datatype: Option<String>,
        lang: Option<String>,
    },
}

impl Value {
    fn node_key(&self) -> Option<String> {
        match self {
            Value::Iri(i) => Some(format!("<{i}>")),
            Value::Blank(b) => Some(format!("_:{b}")),
            Value::Literal { .. } => None,
        }
    }

    /// 字面量的 lexical form（仅字面量有）。
    pub fn lexical(&self) -> Option<&str> {
        match self {
            Value::Literal { value, .. } => Some(value),
            _ => None,
        }
    }

    /// datatype IRI。RDF 1.1 下简单字面量等价于 xsd:string。
    pub fn datatype(&self) -> Option<&str> {
        match self {
            Value::Literal { datatype, .. } => datatype.as_deref(),
            _ => None,
        }
    }
}

/// 极简内存三元组图：只存我们生成的形状与负载需要的部分。
#[derive(Debug, Default, Clone)]
pub struct Graph {
    pub triples: Vec<(Node, String, Value)>,
}

fn conv_subject(s: Subject<'_>) -> Option<Node> {
    match s {
        Subject::NamedNode(n) => Some(Node::Iri(n.iri.to_string())),
        Subject::BlankNode(b) => Some(Node::Blank(b.id.to_string())),
        // RDF-star 三元组作主项：L2 不支持，跳过
        Subject::Triple(_) => None,
    }
}

fn conv_term(t: Term<'_>) -> Option<Value> {
    match t {
        Term::NamedNode(n) => Some(Value::Iri(n.iri.to_string())),
        Term::BlankNode(b) => Some(Value::Blank(b.id.to_string())),
        Term::Literal(l) => Some(match l {
            Literal::Simple { value } => Value::Literal {
                value: value.to_string(),
                datatype: None,
                lang: None,
            },
            Literal::LanguageTaggedString { value, language } => Value::Literal {
                value: value.to_string(),
                datatype: None,
                lang: Some(language.to_string()),
            },
            Literal::Typed { value, datatype } => Value::Literal {
                value: value.to_string(),
                datatype: Some(datatype.iri.to_string()),
                lang: None,
            },
        }),
        // RDF-star 不在 L2 支持范围内，直接跳过（并在文档中声明）。
        Term::Triple(_) => None,
    }
}

impl Graph {
    /// 解析 Turtle 文本。解析失败返回可读错误字符串。
    pub fn parse_turtle(text: &str) -> Result<Self, String> {
        let mut parser = TurtleParser::new(BufReader::new(text.as_bytes()), None);
        let mut triples: Vec<(Node, String, Value)> = Vec::new();
        let mut on_triple = |t: Triple<'_>| -> Result<(), TurtleError> {
            if let (Some(subj), Some(obj)) = (conv_subject(t.subject), conv_term(t.object)) {
                triples.push((subj, t.predicate.iri.to_string(), obj));
            }
            Ok(())
        };
        parser
            .parse_all(&mut on_triple)
            .map_err(|e| e.to_string())?;
        Ok(Self { triples })
    }

    /// 某主某谓的所有宾语。
    pub fn objects(&self, subject: &Node, predicate: &str) -> Vec<&Value> {
        self.triples
            .iter()
            .filter(|(s, p, _)| s == subject && p == predicate)
            .map(|(_, _, o)| o)
            .collect()
    }

    /// 某主某谓的唯一宾语（多于一个时取第一个，形状文件里这些谓词都应是单值）。
    pub fn single_object(&self, subject: &Node, predicate: &str) -> Option<&Value> {
        self.objects(subject, predicate).into_iter().next()
    }

    /// rdf:type 为 class 的所有主项（含 rdfs:subClassOf 闭包，SHACL instance 语义）。
    pub fn instances_of(&self, class: &str) -> Vec<Node> {
        let closure = self.class_closure(class);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (s, p, o) in &self.triples {
            if p == RDF_TYPE {
                if let Value::Iri(iri) = o {
                    if closure.contains(iri) && seen.insert(s.key()) {
                        out.push(s.clone());
                    }
                }
            }
        }
        out
    }

    /// class 及其所有（传递）子类的 IRI 集合。
    pub fn class_closure(&self, class: &str) -> HashSet<String> {
        let mut set = HashSet::new();
        set.insert(class.to_string());
        let mut frontier = vec![class.to_string()];
        while let Some(c) = frontier.pop() {
            for (s, p, o) in &self.triples {
                if p == RDFS_SUBCLASS_OF {
                    if let Value::Iri(parent) = o {
                        if *parent == c {
                            if let Node::Iri(child) = s {
                                if set.insert(child.clone()) {
                                    frontier.push(child.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        set
    }

    /// 取 rdf:List（sh:in 用）的全部元素字面量 lexical form。
    fn list_items(&self, head: &Node) -> Vec<String> {
        let mut items = Vec::new();
        let mut cur = head.clone();
        loop {
            match &cur {
                Node::Iri(i) if i == RDF_NIL => break,
                _ => {
                    let first = self.single_object(&cur, RDF_FIRST);
                    let rest = self.single_object(&cur, RDF_REST);
                    match (first, rest) {
                        (Some(f), Some(r)) => {
                            if let Some(lex) = f.lexical() {
                                items.push(lex.to_string());
                            }
                            match r.node_key().and_then(|k| self.node_by_key(&k)) {
                                Some(next) => cur = next,
                                None => break,
                            }
                        }
                        _ => break,
                    }
                }
            }
        }
        items
    }

    fn node_by_key(&self, key: &str) -> Option<Node> {
        if let Some(rest) = key.strip_prefix('<') {
            Some(Node::Iri(rest.trim_end_matches('>').to_string()))
        } else {
            key.strip_prefix("_:")
                .map(|rest| Node::Blank(rest.to_string()))
        }
    }
}

// ===================== 形状模型 =====================

#[derive(Debug, Default, Clone)]
pub struct PropertyShape {
    pub path: String,
    pub min_count: Option<usize>,
    pub max_count: Option<usize>,
    pub datatype: Option<String>,
    pub in_values: Option<Vec<String>>,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    pub min_inclusive: Option<f64>,
    pub max_inclusive: Option<f64>,
    pub class: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NodeShape {
    pub id: String,
    pub target_class: String,
    pub properties: Vec<PropertyShape>,
}

fn as_usize(v: &Value) -> Option<usize> {
    v.lexical()?.parse::<usize>().ok()
}

fn as_f64(v: &Value) -> Option<f64> {
    v.lexical()?.parse::<f64>().ok()
}

fn as_iri(v: &Value) -> Option<&str> {
    match v {
        Value::Iri(i) => Some(i),
        _ => None,
    }
}

/// 解析单个形状文件，提取全部 NodeShape。
fn extract_shapes(g: &Graph, source: &str) -> AppResult<Vec<NodeShape>> {
    let mut shapes = Vec::new();
    let shape_nodes = g.instances_of(SH_NODE_SHAPE);
    for node in &shape_nodes {
        let target_class = g
            .single_object(node, SH_TARGET_CLASS)
            .and_then(as_iri)
            .ok_or_else(|| {
                AppError::ValidationFailed(format!(
                    "shape in {source}: missing sh:targetClass on {}",
                    node.key()
                ))
            })?
            .to_string();

        let mut properties = Vec::new();
        for ps_value in g.objects(node, SH_PROPERTY) {
            let ps_node = match ps_value {
                Value::Blank(b) => Node::Blank(b.clone()),
                Value::Iri(i) => Node::Iri(i.clone()),
                Value::Literal { .. } => continue, // 形状畸形：跳过，由 L3/CI 报
            };
            let path = g
                .single_object(&ps_node, SH_PATH)
                .and_then(as_iri)
                .ok_or_else(|| {
                    AppError::ValidationFailed(format!(
                        "shape in {source}: sh:property without sh:path on {}",
                        ps_node.key()
                    ))
                })?
                .to_string();

            let mut ps = PropertyShape {
                path,
                ..Default::default()
            };
            if let Some(v) = g.single_object(&ps_node, SH_MIN_COUNT) {
                ps.min_count = as_usize(v);
            }
            if let Some(v) = g.single_object(&ps_node, SH_MAX_COUNT) {
                ps.max_count = as_usize(v);
            }
            if let Some(v) = g.single_object(&ps_node, SH_DATATYPE) {
                ps.datatype = as_iri(v).map(str::to_string);
            }
            if let Some(v) = g.single_object(&ps_node, SH_IN) {
                // sh:in 的值是 rdf:List 头（空白节点）或 rdf:nil
                if let Some(head) = v.node_key().and_then(|k| g.node_by_key(&k)) {
                    ps.in_values = Some(g.list_items(&head));
                }
            }
            if let Some(v) = g.single_object(&ps_node, SH_MIN_LENGTH) {
                ps.min_length = as_usize(v);
            }
            if let Some(v) = g.single_object(&ps_node, SH_MAX_LENGTH) {
                ps.max_length = as_usize(v);
            }
            if let Some(v) = g.single_object(&ps_node, SH_MIN_INCLUSIVE) {
                ps.min_inclusive = as_f64(v);
            }
            if let Some(v) = g.single_object(&ps_node, SH_MAX_INCLUSIVE) {
                ps.max_inclusive = as_f64(v);
            }
            if let Some(v) = g.single_object(&ps_node, SH_CLASS) {
                ps.class = as_iri(v).map(str::to_string);
            }
            if let Some(v) = g.single_object(&ps_node, SH_MESSAGE) {
                ps.message = v.lexical().map(str::to_string);
            }
            properties.push(ps);
        }
        shapes.push(NodeShape {
            id: node.key(),
            target_class,
            properties,
        });
    }
    Ok(shapes)
}

static SHAPE_CACHE: OnceLock<Vec<NodeShape>> = OnceLock::new();

/// 加载全部内嵌形状（进程内缓存一次；形状不随请求变化）。
pub fn load_shapes() -> AppResult<Vec<NodeShape>> {
    if let Some(cached) = SHAPE_CACHE.get() {
        return Ok(cached.clone());
    }
    let mut all = Vec::new();
    for (name, ttl) in SHAPE_FILES {
        let g = Graph::parse_turtle(ttl)
            .map_err(|e| AppError::ValidationFailed(format!("shape {name}: {e}")))?;
        all.extend(extract_shapes(&g, name)?);
    }
    let _ = SHAPE_CACHE.set(all.clone());
    Ok(all)
}

// ===================== 校验 =====================

fn violation(ps: &PropertyShape, default: String) -> Violation {
    Violation {
        path: ps.path.clone(),
        message: ps.message.clone().unwrap_or(default),
    }
}

/// 对图执行全部形状校验，返回违规列表（空 = 通过）。
pub fn validate(graph: &Graph, shapes: &[NodeShape]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for shape in shapes {
        let nodes = graph.instances_of(&shape.target_class);
        for node in &nodes {
            for ps in &shape.properties {
                let values = graph.objects(node, &ps.path);

                if let Some(min) = ps.min_count {
                    if values.len() < min {
                        violations.push(violation(
                            ps,
                            format!(
                                "{}: expected at least {min} value(s), found {}",
                                ps.path,
                                values.len()
                            ),
                        ));
                    }
                }
                if let Some(max) = ps.max_count {
                    if values.len() > max {
                        violations.push(violation(
                            ps,
                            format!(
                                "{}: expected at most {max} value(s), found {}",
                                ps.path,
                                values.len()
                            ),
                        ));
                    }
                }

                for v in &values {
                    if let Some(dt) = &ps.datatype {
                        // RDF 1.1：无 datatype 的简单字面量即 xsd:string
                        let actual = match v.datatype() {
                            Some(d) => Some(d.to_string()),
                            None if matches!(v, Value::Literal { datatype: None, lang: None, .. }) => {
                                Some(XSD_STRING.to_string())
                            }
                            None => None,
                        };
                        if actual.as_deref() != Some(dt.as_str()) {
                            violations.push(violation(
                                ps,
                                format!("{}: expected datatype <{dt}>", ps.path),
                            ));
                        }
                    }
                    if let Some(allowed) = &ps.in_values {
                        match v.lexical() {
                            Some(lex) if allowed.iter().any(|a| a == lex) => {}
                            _ => violations.push(violation(
                                ps,
                                format!("{}: value not in allowed set", ps.path),
                            )),
                        }
                    }
                    if let (Some(min), Some(lex)) = (ps.min_length, v.lexical()) {
                        if lex.chars().count() < min {
                            violations.push(violation(
                                ps,
                                format!("{}: shorter than {min} characters", ps.path),
                            ));
                        }
                    }
                    if let (Some(max), Some(lex)) = (ps.max_length, v.lexical()) {
                        if lex.chars().count() > max {
                            violations.push(violation(
                                ps,
                                format!("{}: longer than {max} characters", ps.path),
                            ));
                        }
                    }
                    if let (Some(min), Some(lex)) = (ps.min_inclusive, v.lexical()) {
                        if let Ok(n) = lex.parse::<f64>() {
                            if n < min {
                                violations.push(violation(
                                    ps,
                                    format!("{}: {n} < minimum {min}", ps.path),
                                ));
                            }
                        }
                    }
                    if let (Some(max), Some(lex)) = (ps.max_inclusive, v.lexical()) {
                        if let Ok(n) = lex.parse::<f64>() {
                            if n > max {
                                violations.push(violation(
                                    ps,
                                    format!("{}: {n} > maximum {max}", ps.path),
                                ));
                            }
                        }
                    }
                    if let Some(class) = &ps.class {
                        // sh:class：值节点必须是该类的 SHACL instance（含子类闭包）
                        let ok = match v {
                            Value::Iri(iri) => {
                                graph
                                    .instances_of(class)
                                    .iter()
                                    .any(|n| matches!(n, Node::Iri(i) if i == iri))
                            }
                            Value::Blank(_) => graph
                                .instances_of(class)
                                .iter()
                                .any(|n| v.node_key().as_deref() == Some(n.key().as_str())),
                            Value::Literal { .. } => false,
                        };
                        if !ok {
                            violations.push(violation(
                                ps,
                                format!("{}: value is not an instance of <{class}>", ps.path),
                            ));
                        }
                    }
                }
            }
        }
    }
    violations
}

/// RDF 负载三元组数量上限（AGENTS.md §4.4：限制三元组数量和 IRI 白名单）。
pub const MAX_RDF_TRIPLES: usize = 2000;

/// PersonCard 专用入口（分身激活，见 api/agent.rs）：在通用形状校验之外，
/// 额外要求负载里**必须**有 soc:Person 实例——否则 targetClass 无命中对象、
/// 违规为空，会把一张空卡误判为合法。
pub fn validate_person_card(rdf_payload: &str) -> AppResult<ValidationResult> {
    let graph = Graph::parse_turtle(rdf_payload)
        .map_err(|e| AppError::ValidationFailed(format!("rdf payload: {e}")))?;
    if graph.triples.len() > MAX_RDF_TRIPLES {
        return Err(AppError::InvalidInput(format!(
            "rdf payload has {} triples, limit is {MAX_RDF_TRIPLES}",
            graph.triples.len()
        )));
    }
    if graph.instances_of(crate::ontology::iri::PERSON).is_empty() {
        return Ok(ValidationResult::with_violations(vec![Violation {
            path: crate::ontology::iri::PERSON.to_string(),
            message: "person card must contain a soc:Person instance".into(),
        }]));
    }
    let shapes = load_shapes()?;
    let violations = validate(&graph, &shapes);
    Ok(ValidationResult::with_violations(violations))
}

/// L2 入口：校验行动携带的 RDF 负载。
///
/// `action_type` 只进审计语义：形状选择由 sh:targetClass 驱动（SHACL 标准语义，
/// 负载里出现哪个类的实例就校验哪个形状），与动作类型解耦。
pub fn validate_local(rdf_payload: &str, action_type: ActionType) -> AppResult<ValidationResult> {
    let graph = Graph::parse_turtle(rdf_payload)
        .map_err(|e| AppError::ValidationFailed(format!("rdf payload: {e}")))?;
    if graph.triples.len() > MAX_RDF_TRIPLES {
        return Err(AppError::InvalidInput(format!(
            "rdf payload has {} triples, limit is {MAX_RDF_TRIPLES}",
            graph.triples.len()
        )));
    }
    let shapes = load_shapes()?;
    let violations = validate(&graph, &shapes);
    let mut result = ValidationResult::with_violations(violations);
    if !result.ok {
        crate::common::logging::warn(
            "shacl_l2_rejected",
            serde_json::json!({ "action_type": format!("{action_type:?}") }),
        );
    }
    result.inferred.clear();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    /// rdfs:subClassOf 的完整 IRI：测试负载没声明 rdfs 前缀，避免再动字面前缀块。
    const SUBCLASS_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";

    const PERSON_OK: &str = r#"
@prefix soc: <http://soc.example/ontology#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
<urn:agent:1> a soc:Person ;
    soc:displayName "测试分身" ;
    soc:status "active" ;
    soc:locatedAt <urn:loc:1> ;
    soc:hasSkill <urn:skill:1> ;
    soc:hasGoal <urn:goal:1> ;
    soc:reputation "0.5"^^xsd:decimal .
<urn:loc:1> a soc:Location .
<urn:skill:1> a soc:Skill .
<urn:goal:1> a soc:Goal .
"#;

    const TRANSACTION_OK: &str = r#"
@prefix econ: <http://soc.example/econ/ontology#> .
@prefix soc: <http://soc.example/ontology#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
<urn:tx:1> a econ:Transaction ;
    soc:affected <urn:res:1> ;
    econ:amount "10.5"^^xsd:decimal ;
    soc:partyTo <urn:a:1> , <urn:a:2> ;
    soc:underOntologyVersion "1.0.0" .
<urn:res:1> a econ:Resource .
<urn:a:1> a soc:Agent .
<urn:a:2> a soc:Agent .
"#;

    fn shapes() -> Vec<NodeShape> {
        load_shapes().expect("embedded shapes must parse")
    }

    /// 解析失败必须带原因：expect 会把 parse error 吞掉，只剩 "payload parses"。
    fn check(payload: &str) -> Vec<Violation> {
        let g = match Graph::parse_turtle(payload) {
            Ok(g) => g,
            Err(e) => panic!("test payload is not valid turtle: {e}\n---\n{payload}\n---"),
        };
        validate(&g, &shapes())
    }

    #[wasm_bindgen_test]
    fn embedded_shapes_parse() {
        let s = shapes();
        // person-card 1 个 + charter.ttl 2 个 + transaction.ttl 1 个
        assert_eq!(s.len(), 4, "{s:?}");
    }

    #[wasm_bindgen_test]
    fn valid_person_card_passes() {
        assert!(check(PERSON_OK).is_empty());
    }

    #[wasm_bindgen_test]
    fn missing_display_name_rejected() {
        let payload = PERSON_OK.replace("    soc:displayName \"测试分身\" ;\n", "");
        let v = check(&payload);
        assert!(v.iter().any(|x| x.path.ends_with("#displayName")), "{v:?}");
    }

    #[wasm_bindgen_test]
    fn over_long_display_name_rejected() {
        let long = "x".repeat(65);
        let payload = PERSON_OK.replace("测试分身", &long);
        let v = check(&payload);
        assert!(
            v.iter()
                .any(|x| x.path.ends_with("#displayName") && x.message.contains("longer")),
            "{v:?}"
        );
    }

    #[wasm_bindgen_test]
    fn too_many_skills_rejected() {
        let payload = PERSON_OK.to_string()
            + "<urn:agent:1> soc:hasSkill <urn:skill:2> , <urn:skill:3> , <urn:skill:4> .\n";
        let v = check(&payload);
        assert!(
            v.iter()
                .any(|x| x.path.ends_with("#hasSkill") && x.message.contains("at most 3")),
            "{v:?}"
        );
    }

    #[wasm_bindgen_test]
    fn status_outside_enum_rejected() {
        let payload = PERSON_OK.replace("\"active\"", "\"flying\"");
        let v = check(&payload);
        assert!(
            v.iter()
                .any(|x| x.path.ends_with("#status") && x.message.contains("allowed set")),
            "{v:?}"
        );
    }

    #[wasm_bindgen_test]
    fn reputation_above_max_rejected() {
        let payload = PERSON_OK.replace("\"0.5\"^^xsd:decimal", "\"1.5\"^^xsd:decimal");
        let v = check(&payload);
        assert!(
            v.iter()
                .any(|x| x.path.ends_with("#reputation") && x.message.contains("maximum")),
            "{v:?}"
        );
    }

    #[wasm_bindgen_test]
    fn subclass_instance_satisfies_sh_class() {
        // soc:Person ⊑ soc:Agent：以 Person 实例填充 soc:Agent 约束应通过。
        // 注意：TBox 必须随负载一并给出——L2 只做闭图推理，不隐式加载 soc.ttl。
        let payload = TRANSACTION_OK
            .replace("<urn:a:1> a soc:Agent .", "<urn:a:1> a soc:Person .")
            .replace("<urn:a:2> a soc:Agent .", "<urn:a:2> a soc:Person .")
            + &format!("soc:Person {SUBCLASS_OF} soc:Agent .\n");
        let v = check(&payload);
        // 只断言 partyTo 的 sh:class 被满足：改成 Person 后 person-card 形状
        // 也会命中这两节点（缺 displayName 等必填），那是另一条约束，与闭包无关。
        let party: Vec<_> = v.iter().filter(|x| x.path.ends_with("#partyTo")).collect();
        assert!(party.is_empty(), "{party:?}");
    }

    #[wasm_bindgen_test]
    fn subclass_closure_is_transitive() {
        // 两层继承也必须展开：Citizen ⊑ Person ⊑ Agent
        let payload = TRANSACTION_OK
            .replace("<urn:a:1> a soc:Agent .", "<urn:a:1> a soc:Citizen .")
            .replace("<urn:a:2> a soc:Agent .", "<urn:a:2> a soc:Citizen .")
            + &format!("soc:Citizen {SUBCLASS_OF} soc:Person .\n")
            + &format!("soc:Person {SUBCLASS_OF} soc:Agent .\n");
        let v = check(&payload);
        let party: Vec<_> = v.iter().filter(|x| x.path.ends_with("#partyTo")).collect();
        assert!(party.is_empty(), "{party:?}");
    }

    #[wasm_bindgen_test]
    fn class_closure_transitive_and_cycle_safe() {
        let ttl = r#"
@prefix soc: <http://soc.example/ontology#> .
soc:Citizen <http://www.w3.org/2000/01/rdf-schema#subClassOf> soc:Person .
soc:Person <http://www.w3.org/2000/01/rdf-schema#subClassOf> soc:Agent .
"#;
        let g = Graph::parse_turtle(ttl).expect("turtle");
        let c = g.class_closure("http://soc.example/ontology#Agent");
        assert!(c.contains("http://soc.example/ontology#Agent"));
        assert!(c.contains("http://soc.example/ontology#Person"));
        assert!(c.contains("http://soc.example/ontology#Citizen"), "{c:?}");

        // 环状继承（A ⊑ B ⊑ A）必须终止而不是死循环——靠闭合集合去重
        let cyclic = r#"
@prefix soc: <http://soc.example/ontology#> .
soc:A <http://www.w3.org/2000/01/rdf-schema#subClassOf> soc:B .
soc:B <http://www.w3.org/2000/01/rdf-schema#subClassOf> soc:A .
"#;
        let g2 = Graph::parse_turtle(cyclic).expect("turtle");
        let c2 = g2.class_closure("http://soc.example/ontology#A");
        assert_eq!(c2.len(), 2, "{c2:?}");
    }

    #[wasm_bindgen_test]
    fn transaction_needs_two_parties_and_version() {
        // 少一方 partyTo：整行删除后 Turtle 仍合法（分号不悬空）
        let payload = TRANSACTION_OK
            .replace("    soc:partyTo <urn:a:1> , <urn:a:2> ;\n", "");
        let v = check(&payload);
        assert!(
            v.iter()
                .any(|x| x.path.ends_with("#partyTo") && x.message.contains("at least 2")),
            "{v:?}"
        );

        // 缺 underOntologyVersion：把三元组合并到上一行收尾，避免留下悬空的 ";"
        let payload = TRANSACTION_OK.replace(
            "    soc:partyTo <urn:a:1> , <urn:a:2> ;\n    soc:underOntologyVersion \"1.0.0\" .\n",
            "    soc:partyTo <urn:a:1> , <urn:a:2> .\n",
        );
        let v = check(&payload);
        assert!(
            v.iter()
                .any(|x| x.path.ends_with("#underOntologyVersion")),
            "{v:?}"
        );
    }

    #[wasm_bindgen_test]
    fn transaction_negative_amount_rejected() {
        let payload = TRANSACTION_OK.replace("\"10.5\"", "\"-1\"");
        let v = check(&payload);
        assert!(
            v.iter()
                .any(|x| x.path.ends_with("#amount") && x.message.contains("minimum")),
            "{v:?}"
        );
    }

    #[wasm_bindgen_test]
    fn charter_self_hascharter_rejected_with_custom_message() {
        let payload = r#"
@prefix gov: <http://soc.example/gov/ontology#> .
@prefix soc: <http://soc.example/ontology#> .
<urn:charter:1> a gov:OrganizationCharter ;
    gov:definesPosition <urn:pos:1> ;
    gov:hasCharter <urn:charter:1> ;
    <http://soc.example/gov/ontology#decisionMethod> "election" .
<urn:pos:1> a soc:Position .
"#;
        let v = check(payload);
        assert!(
            v.iter()
                .any(|x| x.message.contains("章程不应自己挂 hasCharter")),
            "{v:?}"
        );
    }

    #[wasm_bindgen_test]
    fn malformed_turtle_is_error_not_panic() {
        assert!(Graph::parse_turtle("this is not turtle @@@").is_err());
        assert!(validate_local("<<<>>>", ActionType::Move).is_err());
    }

    #[wasm_bindgen_test]
    fn validate_local_ok_for_valid_payload() {
        let r = validate_local(PERSON_OK, ActionType::Work).unwrap();
        assert!(r.ok, "{r:?}");
    }

    #[wasm_bindgen_test]
    fn validate_person_card_accepts_valid_and_rejects_bad_status() {
        let ok = validate_person_card(PERSON_OK).unwrap();
        assert!(ok.ok, "{ok:?}");
        let bad = validate_person_card(&PERSON_OK.replace("\"active\"", "\"flying\"")).unwrap();
        assert!(!bad.ok);
        assert!(bad.violations.iter().any(|v| v.path.ends_with("#status")));
    }

    #[wasm_bindgen_test]
    fn validate_person_card_rejects_card_without_person() {
        // 没有 soc:Person 实例的负载不能因为「无形状命中」被误判合法
        let r = validate_person_card(
            "@prefix soc: <http://soc.example/ontology#> .\n<urn:x:1> a soc:Goal .",
        )
        .unwrap();
        assert!(!r.ok);
        assert!(r.violations.iter().any(|v| v.path.ends_with("#Person")));
    }
}
