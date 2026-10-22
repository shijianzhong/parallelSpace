//! 积分套餐表（服务端定价）。
//!
//! 硬规则：客户端只能提交 **套餐 id**，不能提交金额或积分数。
//! 任何「客户端传金额」的写法都会让支付可被篡改（改 body 换 1 分钱买 7000 积分），
//! 所以价格表是常驻内存的常量，且必须有单测钉住「未知 id 一律拒绝」。

/// 一个可售套餐。价格单位：分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreditPackage {
    pub id: &'static str,
    /// 积分数量
    pub credits: i64,
    /// 价格（分）
    pub amount_cents: i64,
}

/// 当前在售套餐。调整价格只改这里，不动客户端。
pub const CREDIT_PACKAGES: &[CreditPackage] = &[
    CreditPackage {
        id: "starter",
        credits: 500,
        amount_cents: 1000, // ¥10.00
    },
    CreditPackage {
        id: "standard",
        credits: 3000,
        amount_cents: 5000, // ¥50.00
    },
    CreditPackage {
        id: "pro",
        credits: 7000,
        amount_cents: 10000, // ¥100.00
    },
];

/// 按 id 查套餐；未知 id 返回 None（调用方必须据此拒绝，不做模糊匹配）。
pub fn find_package(id: &str) -> Option<&'static CreditPackage> {
    CREDIT_PACKAGES.iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    #[wasm_bindgen_test]
    fn lookup_is_exact_match_only() {
        assert!(find_package("starter").is_some());
        assert!(find_package("").is_none());
        assert!(find_package("STARTER").is_none());
        assert!(find_package("starter ").is_none());
        assert!(find_package("unknown").is_none());
    }

    #[wasm_bindgen_test]
    fn packages_are_sane() {
        for p in CREDIT_PACKAGES {
            assert!(p.credits > 0, "{p:?}");
            assert!(p.amount_cents > 0, "{p:?}");
            // 单价必须为正且不超过 1 元/积分（防手滑把分写成元）
            let unit = p.amount_cents as f64 / p.credits as f64;
            assert!(unit > 0.0 && unit <= 100.0, "{p:?} unit={unit}");
        }
        // id 唯一
        for (i, a) in CREDIT_PACKAGES.iter().enumerate() {
            for b in &CREDIT_PACKAGES[i + 1..] {
                assert_ne!(a.id, b.id);
            }
        }
    }
}
