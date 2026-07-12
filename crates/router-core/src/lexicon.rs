//! The heuristic's fixed vocabulary (V2-P2-3), ported from the V1
//! `complexity_router` and extended with Chinese equivalents.
//!
//! Constants, deliberately. The score these lists feed decides routing, and a
//! logged decision must replay to the same answer against the config of the
//! day (`C3#3`) — a vocabulary that lived in configuration would be a second,
//! invisible input the audit log does not capture. Editing a list is a crate
//! release, which is exactly the ceremony a behavior change to every
//! deployment's routing deserves. Only *counts* of hits leave this module;
//! the text itself is read and dropped, per [`crate::RequestFeatures`]'s
//! contract.

/// Phrases that ask for deliberate reasoning, such as "step by step" or "pros and cons".
pub(crate) const REASONING_MARKERS: &[&str] = &[
    // English (V1 list, verbatim).
    "step by step",
    "think through",
    "analyze",
    "reason about",
    "explain why",
    "compare and contrast",
    "evaluate",
    "what are the tradeoffs",
    "pros and cons",
    "critically",
    "systematically",
    "break down",
    "deep dive",
    // Chinese.
    "一步一步",
    "逐步",
    "推理",
    "分析",
    "论证",
    "解释为什么",
    "比较",
    "权衡",
    "利弊",
    "优缺点",
    "系统地",
    "深入",
    "拆解",
    "评估",
];

/// Tokens that suggest the request is about code even without a fence.
/// Code identifiers are latin regardless of the author's language, so this
/// list has no Chinese half.
pub(crate) const CODE_KEYWORDS: &[&str] = &[
    "function",
    "class",
    "struct",
    "impl",
    "async",
    "await",
    "def",
    "import",
    "return",
    "const",
    "enum",
    "trait",
    "interface",
    "select",
    "insert",
    "create table",
    "docker",
    "kubectl",
    "git rebase",
    "curl",
    "grep",
    "sudo",
    // Chinese framing around code.
    "代码",
    "函数",
    "报错",
    "编译",
];

/// Domain vocabulary that correlates with genuinely hard requests.
pub(crate) const TECHNICAL_TERMS: &[&str] = &[
    "architecture",
    "distributed",
    "encryption",
    "algorithm",
    "optimization",
    "concurrency",
    "parallelism",
    "microservice",
    "kubernetes",
    "database schema",
    "machine learning",
    "neural network",
    "transformer",
    "gradient",
    "backpropagation",
    "consensus",
    "replication",
    "sharding",
    "zero-knowledge",
    "homomorphic",
    "byzantine",
    "mutex",
    "semaphore",
    "deadlock",
    "race condition",
    "memory leak",
    "garbage collection",
    "compiler",
    "parser",
    "lexer",
    "type system",
    "monomorphization",
    "polymorphism",
    "dependency injection",
    // Chinese.
    "架构",
    "分布式",
    "加密",
    "算法",
    "优化",
    "并发",
    "并行",
    "微服务",
    "数据库",
    "机器学习",
    "神经网络",
    "共识",
    "分片",
    "死锁",
    "竞态",
    "内存泄漏",
    "垃圾回收",
    "编译器",
    "类型系统",
    "多态",
    "依赖注入",
];

/// Phrases that argue the request is easy. The only list that SUBTRACTS.
pub(crate) const SIMPLE_INDICATORS: &[&str] = &[
    "what is",
    "define",
    "hello",
    "thanks",
    "thank you",
    "who is",
    "when was",
    "where is",
    "how old",
    "what color",
    "good morning",
    "good night",
    "goodbye",
    "how are you",
    "what time",
    "what day",
    "translate",
    // Chinese.
    "是什么",
    "什么是",
    "你好",
    "谢谢",
    "多谢",
    "是谁",
    "什么时候",
    "在哪里",
    "多大",
    "什么颜色",
    "早上好",
    "晚安",
    "再见",
    "你好吗",
    "几点",
    "星期几",
    "翻译",
];

/// Math and formal-logic vocabulary.
pub(crate) const MATH_TERMS: &[&str] = &[
    "prove",
    "calculate",
    "derive",
    "equation",
    "integral",
    "derivative",
    "probability",
    "theorem",
    "conjecture",
    "polynomial",
    "matrix",
    "eigenvalue",
    "linear algebra",
    "differential",
    // Chinese.
    "证明",
    "计算",
    "推导",
    "方程",
    "积分",
    "导数",
    "概率",
    "定理",
    "猜想",
    "多项式",
    "矩阵",
    "特征值",
    "线性代数",
    "微分",
];

/// Creative-writing vocabulary.
pub(crate) const CREATIVE_TERMS: &[&str] = &[
    "write a story",
    "write a poem",
    "essay",
    "creative",
    "fiction",
    "narrative",
    "screenplay",
    "dialogue",
    "character development",
    // Chinese.
    "写一个故事",
    "写个故事",
    "写一首诗",
    "写首诗",
    "作文",
    "散文",
    "小说",
    "剧本",
    "人物塑造",
    "创作",
];

/// Structured-output vocabulary, scanned over SYSTEM text only (dim 10).
pub(crate) const FORMAT_TERMS: &[&str] = &[
    "json",
    "yaml",
    "xml",
    "structured",
    "schema",
    "format",
    // Chinese.
    "结构化",
    "格式",
];

/// Whether lowercased `text` contains `phrase`.
///
/// A pure-ASCII single word must match on word boundaries ("hi" must not fire
/// inside "this" — V1's rule, kept verbatim). Anything else — multi-word
/// phrases, and any phrase containing a non-ASCII character — is a substring
/// match: CJK has no word boundaries to respect.
pub(crate) fn has_phrase(text: &str, phrase: &str) -> bool {
    if phrase.contains(' ') || !phrase.is_ascii() {
        text.contains(phrase)
    } else {
        text.split(|c: char| !c.is_alphanumeric())
            .any(|word| word == phrase)
    }
}

/// How many of `phrases` appear in lowercased `text`. One point per list
/// entry at most — repetition of a single phrase is not extra evidence.
pub(crate) fn count_matches(text: &str, phrases: &[&str]) -> u32 {
    let hits = phrases.iter().filter(|p| has_phrase(text, p)).count();
    u32::try_from(hits).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_single_words_respect_word_boundaries() {
        assert!(has_phrase("please analyze this", "analyze"));
        assert!(!has_phrase("the analyzer output", "analyze"));
        assert!(has_phrase(
            "compare and contrast the two",
            "compare and contrast"
        ));
    }

    #[test]
    fn cjk_phrases_match_by_substring() {
        assert!(has_phrase("请证明这个定理", "证明"));
        assert!(has_phrase("帮我写一首诗吧", "写一首诗"));
        assert!(!has_phrase("今天天气不错", "证明"));
    }

    #[test]
    fn counting_is_per_list_entry_not_per_occurrence() {
        assert_eq!(
            count_matches("analyze analyze analyze", REASONING_MARKERS),
            1
        );
        assert_eq!(
            count_matches("请系统地分析并评估利弊", REASONING_MARKERS),
            4
        );
    }
}
