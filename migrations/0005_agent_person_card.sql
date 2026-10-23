-- 0005：PersonCard 落库（G5）。注册时可选携带 PersonCard（RDF/Turtle），
-- 先过 SHACL（PersonCardShape）才落 agents.person_card_rdf；激活时请求未带
-- card 则优先用已存 card。LLM 结构化生成仍为 TODO，等 G12 上游定版后接。
ALTER TABLE agents ADD COLUMN person_card_rdf TEXT;
