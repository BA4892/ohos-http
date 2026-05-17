use regex::Regex;
use crate::config::RewriteRule;

/// URL重写引擎
pub struct RewriteEngine {
    rules: Vec<CompiledRule>,
}

struct CompiledRule {
    from: Regex,
    to: String,
}

impl RewriteEngine {
    pub fn new(rules: &[RewriteRule]) -> Self {
        let compiled = rules
            .iter()
            .filter_map(|r| {
                Regex::new(&r.from).ok().map(|re| CompiledRule {
                    from: re,
                    to: r.to.clone(),
                })
            })
            .collect();
        RewriteEngine { rules: compiled }
    }

    /// 尝试重写路径，返回重写后的路径，如果无匹配则返回 None
    pub fn rewrite(&self, path: &str) -> Option<String> {
        for rule in &self.rules {
            if let Some(caps) = rule.from.captures(path) {
                let mut result = rule.to.clone();
                // 替换 $1, $2 等捕获组
                for (i, cap) in caps.iter().enumerate().skip(1) {
                    if let Some(m) = cap {
                        result = result.replace(&format!("${}", i), m.as_str());
                    }
                }
                return Some(result);
            }
        }
        None
    }
}
