//! L2 轻量推理：reasonable（OWL 2 RL 子集）。
//!
//! 分层（SAD §5）：L2 在行动提交时 < 50ms 快速拒绝，L3 完整 OWL 2 DL 推理走外部
//! open-ontologies HTTP 服务（governance/validator.rs 的 validate_remote）。
//!
//! 用法：本体（TBox，内嵌 ontology/*.ttl）+ 数据（行动负载）一次性装载，
//! `reason()` 后经 `view_output()` 读推论。注意 reasonable 内部用 Rc/RefCell，
//! 实例不是 Send——Workers 单线程模型下无影响，但不要跨线程搬运。

use std::cell::RefCell;
use std::io::BufReader;

use oxrdf::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Term, Triple};
use reasonable::reasoner::Reasoner;
use rio_api::model::{Literal as RioLiteral, Subject as RioSubject, Term as RioTerm, Triple as RioTriple};
use rio_api::parser::TriplesParser;
use rio_turtle::{TurtleError, TurtleParser};

use crate::common::error::{AppError, AppResult};
use crate::ontology::iri;

/// 内嵌本体（TBox）。SHACL 形状不参与推理。
pub const ONTOLOGY_FILES: &[(&str, &str)] = &[
    ("soc", include_str!("../../ontology/soc.ttl")),
    ("gov", include_str!("../../ontology/gov.ttl")),
    ("econ", include_str!("../../ontology/econ.ttl")),
    ("geo", include_str!("../../ontology/geo.ttl")),
    ("time", include_str!("../../ontology/time.ttl")),
];

pub struct Owl2RlReasoner {
    inner: RefCell<Reasoner>,
}

fn collect_triples(turtle: &str, out: &mut Vec<Triple>) -> AppResult<()> {
    let mut parser = TurtleParser::new(BufReader::new(turtle.as_bytes()), None);
    let mut on_triple = |t: RioTriple<'_>| -> Result<(), TurtleError> {
        if let Some(t) = to_oxrdf(t) {
            out.push(t);
        }
        Ok(())
    };
    parser
        .parse_all(&mut on_triple)
        .map_err(|e| AppError::OntologyEngine(format!("turtle parse: {e}")))?;
    Ok(())
}

/// 只解析 Turtle 文本为三元组（不装载内嵌本体、不做推理）。
///
/// 供治理影响分析（governance::ontology_admin::analyze_impact）复用与
/// `Owl2RlReasoner::build` 完全相同的解析路径，避免出现第二套解析语义。
pub fn parse_turtle(turtle: &str) -> AppResult<Vec<Triple>> {
    let mut triples = Vec::new();
    collect_triples(turtle, &mut triples)?;
    Ok(triples)
}

fn to_oxrdf(t: RioTriple<'_>) -> Option<Triple> {
    let subject: NamedOrBlankNode = match t.subject {
        RioSubject::NamedNode(n) => NamedNode::new_unchecked(n.iri).into(),
        RioSubject::BlankNode(b) => BlankNode::new_unchecked(b.id).into(),
        // RDF-star 三元组作主项：不支持，跳过
        RioSubject::Triple(_) => return None,
    };
    let predicate = NamedNode::new_unchecked(t.predicate.iri);
    let object: Term = match t.object {
        RioTerm::NamedNode(n) => NamedNode::new_unchecked(n.iri).into(),
        RioTerm::BlankNode(b) => BlankNode::new_unchecked(b.id).into(),
        RioTerm::Literal(l) => match l {
            RioLiteral::Simple { value } => Literal::new_simple_literal(value).into(),
            RioLiteral::LanguageTaggedString { value, language } => {
                Literal::new_language_tagged_literal_unchecked(value, language).into()
            }
            RioLiteral::Typed { value, datatype } => {
                Literal::new_typed_literal(value, NamedNode::new_unchecked(datatype.iri)).into()
            }
        },
        // RDF-star 不在支持范围
        RioTerm::Triple(_) => Literal::new_simple_literal("__rdf_star_unsupported__").into(),
    };
    Some(Triple::new(subject, predicate, object))
}

impl Owl2RlReasoner {
    /// 装载内嵌本体 + 数据负载并完成一轮 OWL 2 RL 推理。
    pub fn build(data_turtle: &str) -> AppResult<Self> {
        let mut triples = Vec::new();
        for (_, ttl) in ONTOLOGY_FILES {
            collect_triples(ttl, &mut triples)?;
        }
        collect_triples(data_turtle, &mut triples)?;
        let mut reasoner = Reasoner::new();
        reasoner.load_triples(triples);
        reasoner.reason();
        Ok(Self {
            inner: RefCell::new(reasoner),
        })
    }

    /// 本轮推理产出的推论三元组（N-Triples 形式）。
    pub fn inferred(&self) -> Vec<String> {
        self.inner
            .borrow()
            .view_output()
            .iter()
            .map(|t| t.to_string())
            .collect()
    }

    /// 一致性：图中出现 owl:Nothing 的实例即不一致（OWL 2 RL 标准判法）。
    pub fn is_consistent(&self) -> bool {
        !self.inner.borrow().get_triples().iter().any(|t| {
            t.predicate.as_str() == iri::RDF_TYPE && t.object.to_string() == format!("<{}>", iri::OWL_NOTHING)
        })
    }

    /// 某三元组是否被本体蕴含（装载后查询全量三元组）。
    pub fn entails(&self, s: &str, p: &str, o: &str) -> bool {
        self.inner.borrow().get_triples().iter().any(|t| {
            t.subject.to_string() == format!("<{s}>")
                && t.predicate.as_str() == p
                && t.object.to_string() == format!("<{o}>")
        })
    }
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    /// 内嵌本体必须全部能被 Turtle 解析——任一 .ttl 写错都会在 build 时炸所有调用方。
    #[wasm_bindgen_test]
    fn embedded_ontology_files_parse() {
        let mut triples = Vec::new();
        let mut total = 0usize;
        for (name, ttl) in ONTOLOGY_FILES {
            match collect_triples(ttl, &mut triples) {
                Ok(()) => {
                    assert!(!ttl.trim().is_empty(), "{name}.ttl is empty");
                    total += 1;
                }
                Err(e) => panic!("{name}.ttl parse failed: {e}"),
            }
        }
        assert_eq!(total, ONTOLOGY_FILES.len());
        assert!(!triples.is_empty(), "ontology produced no triples");
    }

    /// 用病因可读的方式断言 build 成功（.expect 只给了 Result 丢弃了原因）。
    fn build_ok(data: &str) -> Owl2RlReasoner {
        match Owl2RlReasoner::build(data) {
            Ok(r) => r,
            Err(e) => panic!("build failed: {e}"),
        }
    }

    /// soc.ttl 公理：soc:holdsPosition rdfs:subPropertyOf soc:memberOf 的传递场景。
    /// 用 rdfs:subClassOf 传递闭包验证 reasonable 真实在工作。
    #[wasm_bindgen_test]
    fn reasoner_infers_subclass_transitivity() {
        let data = r#"
@prefix soc: <http://soc.example/ontology#> .
<urn:x:1> a soc:Person .
"#;
        let r = build_ok(data);
        assert!(r.is_consistent(), "ontology itself must be consistent");
        // Person ⊑ Agent ⊑ prov:Agent ⇒ x 也是 soc:Agent
        assert!(
            r.entails("urn:x:1", iri::RDF_TYPE, iri::AGENT),
            "inferred: {:?}",
            r.inferred()
        );
    }

    #[wasm_bindgen_test]
    fn reasoner_detects_nothing_inconsistency() {
        // 直接把实例声明为 owl:Nothing ⇒ 不一致
        let data = r#"
@prefix owl: <http://www.w3.org/2002/07/owl#> .
<urn:x:bad> a owl:Nothing .
"#;
        let r = build_ok(data);
        assert!(!r.is_consistent());
    }

    #[wasm_bindgen_test]
    fn build_rejects_bad_turtle() {
        assert!(Owl2RlReasoner::build("<<<not turtle").is_err());
    }

    #[wasm_bindgen_test]
    fn parse_turtle_returns_triples_without_reasoning() {
        let ttl = "@prefix soc: <http://soc.example/ontology#> .\n<urn:x:1> a soc:Person .\n";
        let triples = parse_turtle(ttl).expect("parse");
        assert_eq!(triples.len(), 1);
        assert_eq!(triples[0].subject.to_string(), "<urn:x:1>");
        // 坏 Turtle 同样报错，与 build 路径一致
        assert!(parse_turtle("<<<not turtle").is_err());
    }
}
