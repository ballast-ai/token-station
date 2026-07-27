use std::io::Read;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use token_station_cli::config::EgressConfig;
use token_station_cli::secrets::SecretStore;
use token_station_protocol::{
    Auth, CapabilityState, HttpMethod, HttpRequestDescriptor, ProviderApi, ProviderConfig,
    ProviderEndpoint, SecretRef,
};

pub(crate) const CURATED_AT: &str = "2026-07-27";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FreeOfferKind {
    Recurring,
    Trial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderRegion {
    China,
    Global,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OveragePolicy {
    HardStop,
    RateLimited,
    UserMustEnableGuard,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct FreeModelPreset {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) tool: CapabilityState,
    pub(crate) vision: CapabilityState,
    pub(crate) json_schema: CapabilityState,
    pub(crate) context_window: u32,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct FreeProviderPreset {
    pub(crate) id: &'static str,
    pub(crate) upstream_name: &'static str,
    pub(crate) label: &'static str,
    pub(crate) short_label: &'static str,
    pub(crate) base_url: &'static str,
    pub(crate) offer_kind: FreeOfferKind,
    pub(crate) region: ProviderRegion,
    pub(crate) tags: &'static [&'static str],
    pub(crate) free_note: &'static str,
    pub(crate) key_instruction: &'static str,
    pub(crate) application_url: &'static str,
    pub(crate) docs_url: &'static str,
    pub(crate) verified_at: &'static str,
    pub(crate) overage_policy: OveragePolicy,
    pub(crate) models: &'static [FreeModelPreset],
}

const DECLARED: CapabilityState = CapabilityState::Declared;
const UNKNOWN: CapabilityState = CapabilityState::Unknown;

const SILICONFLOW_MODELS: &[FreeModelPreset] = &[
    model("deepseek-ai/DeepSeek-V3.2", "DeepSeek V3.2", 128_000),
    model("Qwen/Qwen3-32B", "Qwen3 32B", 128_000),
    model("moonshotai/Kimi-K2.5", "Kimi K2.5", 128_000),
    model("zai-org/GLM-4.7", "GLM 4.7", 128_000),
];
const MODELSCOPE_MODELS: &[FreeModelPreset] = &[
    model("Qwen/Qwen3-32B", "Qwen3 32B", 32_768),
    model("Qwen/Qwen3-235B-A22B-Instruct-2507", "Qwen3 235B", 128_000),
];
const ALIBABA_MODELS: &[FreeModelPreset] = &[
    model("qwen-turbo", "Qwen Turbo", 1_000_000),
    model("qwen-plus", "Qwen Plus", 1_000_000),
];
const TENCENT_MODELS: &[FreeModelPreset] = &[
    model("hunyuan-lite", "Hunyuan Lite", 32_000),
    model("hunyuan-pro", "Hunyuan Pro", 32_000),
];
const GEMINI_MODELS: &[FreeModelPreset] = &[
    vision_model("gemini-2.5-flash", "Gemini 2.5 Flash", 1_000_000),
    vision_model("gemini-2.5-flash-lite", "Gemini 2.5 Flash-Lite", 1_000_000),
    vision_model(
        "gemini-3-flash-preview",
        "Gemini 3 Flash Preview",
        1_000_000,
    ),
];
const GROQ_MODELS: &[FreeModelPreset] = &[
    model("openai/gpt-oss-120b", "GPT-OSS 120B", 131_072),
    model("openai/gpt-oss-20b", "GPT-OSS 20B", 131_072),
    model("llama-3.3-70b-versatile", "Llama 3.3 70B", 128_000),
    model("qwen/qwen3-32b", "Qwen3 32B", 131_072),
];
const MISTRAL_MODELS: &[FreeModelPreset] = &[
    model("mistral-small-latest", "Mistral Small", 128_000),
    model("devstral-latest", "Devstral", 128_000),
    model("codestral-latest", "Codestral", 256_000),
];
const SAMBANOVA_MODELS: &[FreeModelPreset] = &[
    model("DeepSeek-V3.2", "DeepSeek V3.2", 128_000),
    model("Meta-Llama-3.3-70B-Instruct", "Llama 3.3 70B", 128_000),
    model("gpt-oss-120b", "GPT-OSS 120B", 131_072),
];
const OPENROUTER_MODELS: &[FreeModelPreset] =
    &[model("openrouter/free", "Free Models Router", 128_000)];
const GITHUB_MODELS: &[FreeModelPreset] = &[
    model("deepseek/DeepSeek-R1-0528", "DeepSeek R1 0528", 128_000),
    model("meta/Llama-3.3-70B-Instruct", "Llama 3.3 70B", 128_000),
    vision_model(
        "microsoft/Phi-4-multimodal-instruct",
        "Phi-4 Multimodal",
        128_000,
    ),
];
const COHERE_MODELS: &[FreeModelPreset] = &[
    model(
        "command-a-reasoning-08-2025",
        "Command A Reasoning",
        256_000,
    ),
    vision_model("command-a-vision-07-2025", "Command A Vision", 128_000),
    model("command-r7b-12-2024", "Command R7B", 128_000),
];
const HUGGING_FACE_MODELS: &[FreeModelPreset] = &[
    model("meta-llama/Llama-3.1-8B-Instruct", "Llama 3.1 8B", 128_000),
    model("Qwen/Qwen2.5-7B-Instruct", "Qwen 2.5 7B", 32_768),
    model("mistralai/Mistral-7B-Instruct-v0.3", "Mistral 7B", 32_768),
];
const NVIDIA_MODELS: &[FreeModelPreset] = &[
    model("openai/gpt-oss-120b", "GPT-OSS 120B", 131_072),
    model("openai/gpt-oss-20b", "GPT-OSS 20B", 131_072),
    model(
        "nvidia/nemotron-3-super-120b-a12b",
        "Nemotron 3 Super",
        262_144,
    ),
];

const PRESETS: &[FreeProviderPreset] = &[
    preset(
        "siliconflow",
        "siliconflow_free",
        "硅基流动 SiliconFlow",
        "SF",
        "https://api.siliconflow.cn/v1",
        FreeOfferKind::Recurring,
        ProviderRegion::China,
        &["长期免费", "中国可用", "开源模型"],
        "仅展示官方免费模型；免费模型有速率限制。",
        "在 SiliconFlow 控制台创建 API Key，然后粘贴到这里。",
        "https://cloud.siliconflow.cn/account/ak",
        "https://docs.siliconflow.cn/cn/userguide/quickstart",
        OveragePolicy::RateLimited,
        SILICONFLOW_MODELS,
    ),
    preset(
        "modelscope",
        "modelscope_free",
        "魔搭 ModelScope",
        "MS",
        "https://api-inference.modelscope.cn/v1",
        FreeOfferKind::Recurring,
        ProviderRegion::China,
        &["长期免费", "中国可用", "API-Inference"],
        "使用 ModelScope API-Inference 免费调用范围，模型可用性会随平台调整。",
        "登录 ModelScope，在访问令牌页面创建 Token。",
        "https://modelscope.cn/my/myaccesstoken",
        "https://modelscope.cn/docs/model-service/API-Inference/intro",
        OveragePolicy::RateLimited,
        MODELSCOPE_MODELS,
    ),
    preset(
        "alibaba_model_studio",
        "alibaba_free",
        "阿里云百炼",
        "AL",
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
        FreeOfferKind::Trial,
        ProviderRegion::China,
        &["试用额度", "中国可用", "需额度保护"],
        "新用户免费额度；必须在百炼控制台确认不会超额转后付费。",
        "在阿里云百炼控制台创建 API Key，并确认免费额度保护。",
        "https://bailian.console.aliyun.com/",
        "https://help.aliyun.com/zh/model-studio/new-free-quota",
        OveragePolicy::UserMustEnableGuard,
        ALIBABA_MODELS,
    ),
    preset(
        "tencent_hunyuan",
        "tencent_free",
        "腾讯混元",
        "HY",
        "https://api.hunyuan.cloud.tencent.com/v1",
        FreeOfferKind::Trial,
        ProviderRegion::China,
        &["试用额度", "中国可用", "需额度保护"],
        "使用官方免费资源包；Token Station 不会替你开启后付费。",
        "在腾讯混元控制台创建 API Key，并确认免费资源包到期行为。",
        "https://console.cloud.tencent.com/hunyuan/api-key",
        "https://cloud.tencent.com/document/product/1729/111007",
        OveragePolicy::UserMustEnableGuard,
        TENCENT_MODELS,
    ),
    preset(
        "gemini",
        "gemini_free",
        "Google Gemini API",
        "GM",
        "https://generativelanguage.googleapis.com/v1beta/openai",
        FreeOfferKind::Recurring,
        ProviderRegion::Global,
        &["长期免费", "全球平台", "多模态"],
        "仅收录 Gemini API Free Tier 模型，受地区和速率限制。",
        "在 Google AI Studio 创建 Gemini API Key。",
        "https://aistudio.google.com/app/apikey",
        "https://ai.google.dev/gemini-api/docs/openai",
        OveragePolicy::UserMustEnableGuard,
        GEMINI_MODELS,
    ),
    preset(
        "groq",
        "groq_free",
        "Groq",
        "GQ",
        "https://api.groq.com/openai/v1",
        FreeOfferKind::Recurring,
        ProviderRegion::Global,
        &["长期免费", "全球平台", "低延迟"],
        "Free Plan 按模型限速，达到限制后请求会被拒绝。",
        "在 Groq Console 的 API Keys 页面创建 Key。",
        "https://console.groq.com/keys",
        "https://console.groq.com/docs/openai",
        OveragePolicy::RateLimited,
        GROQ_MODELS,
    ),
    preset(
        "mistral",
        "mistral_free",
        "Mistral AI",
        "MI",
        "https://api.mistral.ai/v1",
        FreeOfferKind::Recurring,
        ProviderRegion::Global,
        &["长期免费", "全球平台", "Free mode"],
        "使用 Mistral Experiment 免费模式，受速率和服务限制。",
        "在 Mistral La Plateforme 创建 API Key，并选择 Experiment 方案。",
        "https://console.mistral.ai/api-keys/",
        "https://docs.mistral.ai/admin/billing-usage/usage-limits",
        OveragePolicy::RateLimited,
        MISTRAL_MODELS,
    ),
    preset(
        "sambanova",
        "sambanova_free",
        "SambaNova Cloud",
        "SN",
        "https://api.sambanova.ai/v1",
        FreeOfferKind::Recurring,
        ProviderRegion::Global,
        &["长期免费", "全球平台", "高速推理"],
        "Free Tier 无需付款方式，按日额度和速率限制。",
        "注册 SambaNova Cloud 后，在 API Keys 页面创建 Key。",
        "https://cloud.sambanova.ai/apis",
        "https://docs.sambanova.ai/docs/en/models/rate-limits",
        OveragePolicy::HardStop,
        SAMBANOVA_MODELS,
    ),
    preset(
        "openrouter",
        "openrouter_free",
        "OpenRouter",
        "OR",
        "https://openrouter.ai/api/v1",
        FreeOfferKind::Recurring,
        ProviderRegion::Global,
        &["长期免费", "全球平台", "免费路由"],
        "固定使用 openrouter/free，只允许路由到价格为零的模型。",
        "在 OpenRouter Keys 页面创建 API Key。",
        "https://openrouter.ai/settings/keys",
        "https://openrouter.ai/docs/faq",
        OveragePolicy::RateLimited,
        OPENROUTER_MODELS,
    ),
    preset(
        "github_models",
        "github_models_free",
        "GitHub Models",
        "GH",
        "https://models.github.ai/inference",
        FreeOfferKind::Recurring,
        ProviderRegion::Global,
        &["长期免费", "全球平台", "GitHub PAT"],
        "用于原型开发的免费限额；需要带 models:read 权限的 PAT。",
        "创建仅包含 models:read 权限的 GitHub Fine-grained PAT。",
        "https://github.com/settings/personal-access-tokens/new",
        "https://docs.github.com/en/github-models/use-github-models/prototyping-with-ai-models",
        OveragePolicy::HardStop,
        GITHUB_MODELS,
    ),
    preset(
        "cohere",
        "cohere_free",
        "Cohere",
        "CO",
        "https://api.cohere.ai/compatibility/v1",
        FreeOfferKind::Trial,
        ProviderRegion::Global,
        &["试用额度", "全球平台", "Trial Key"],
        "Trial Key 免费但受速率限制，不适用于正式生产负载。",
        "登录 Cohere Dashboard，复制默认 Trial API Key。",
        "https://dashboard.cohere.com/api-keys",
        "https://docs.cohere.com/docs/going-live",
        OveragePolicy::HardStop,
        COHERE_MODELS,
    ),
    preset(
        "hugging_face",
        "hugging_face_free",
        "Hugging Face",
        "HF",
        "https://router.huggingface.co/v1",
        FreeOfferKind::Recurring,
        ProviderRegion::Global,
        &["长期免费", "全球平台", "月度额度"],
        "免费账户包含每月推理额度；只选择支持聊天补全的模型。",
        "在 Hugging Face Access Tokens 页面创建只读 Token。",
        "https://huggingface.co/settings/tokens/new",
        "https://huggingface.co/docs/inference-providers/pricing",
        OveragePolicy::UserMustEnableGuard,
        HUGGING_FACE_MODELS,
    ),
    preset(
        "nvidia",
        "nvidia_free",
        "NVIDIA API Catalog",
        "NV",
        "https://integrate.api.nvidia.com/v1",
        FreeOfferKind::Recurring,
        ProviderRegion::Global,
        &["长期免费", "全球平台", "开发用途"],
        "build.nvidia.com 托管 API 的免费开发额度，不是本地 NIM 或企业试用。",
        "登录 build.nvidia.com，打开模型页面并点击 Get API Key。",
        "https://build.nvidia.com/",
        "https://docs.api.nvidia.com/nim/docs/api-quickstart",
        OveragePolicy::RateLimited,
        NVIDIA_MODELS,
    ),
];

const fn model(id: &'static str, label: &'static str, context_window: u32) -> FreeModelPreset {
    FreeModelPreset {
        id,
        label,
        tool: UNKNOWN,
        vision: UNKNOWN,
        json_schema: UNKNOWN,
        context_window,
    }
}

const fn vision_model(
    id: &'static str,
    label: &'static str,
    context_window: u32,
) -> FreeModelPreset {
    FreeModelPreset {
        id,
        label,
        tool: UNKNOWN,
        vision: DECLARED,
        json_schema: UNKNOWN,
        context_window,
    }
}

#[allow(clippy::too_many_arguments)]
const fn preset(
    id: &'static str,
    upstream_name: &'static str,
    label: &'static str,
    short_label: &'static str,
    base_url: &'static str,
    offer_kind: FreeOfferKind,
    region: ProviderRegion,
    tags: &'static [&'static str],
    free_note: &'static str,
    key_instruction: &'static str,
    application_url: &'static str,
    docs_url: &'static str,
    overage_policy: OveragePolicy,
    models: &'static [FreeModelPreset],
) -> FreeProviderPreset {
    FreeProviderPreset {
        id,
        upstream_name,
        label,
        short_label,
        base_url,
        offer_kind,
        region,
        tags,
        free_note,
        key_instruction,
        application_url,
        docs_url,
        verified_at: CURATED_AT,
        overage_policy,
        models,
    }
}

pub(crate) fn presets() -> &'static [FreeProviderPreset] {
    PRESETS
}

pub(crate) fn find(id: &str) -> Option<&'static FreeProviderPreset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

pub(crate) fn validate_stored_provider(name: &str, provider: &Value) -> Result<(), String> {
    let preset = PRESETS
        .iter()
        .find(|preset| preset.upstream_name == name)
        .ok_or_else(|| format!("免费供应商 `{name}` 已不在当前内置目录中，请删除后重新选择"))?;
    if provider["provider"].as_str() != Some("openai-compatible")
        || provider["base_url"].as_str() != Some(preset.base_url)
        || provider["access_tier"].as_str() != Some("free")
        || provider["auth"]["slot"].as_str() != Some("provider_api_key")
        || provider["auth"]["keyring"].as_bool() != Some(true)
    {
        return Err(format!(
            "免费供应商 `{name}` 与当前内置目录身份不一致，请删除后重新验证"
        ));
    }
    let models = provider["models"]
        .as_array()
        .filter(|models| !models.is_empty())
        .ok_or_else(|| format!("免费供应商 `{name}` 没有可用模型"))?;
    let mut seen = std::collections::BTreeSet::new();
    for stored in models {
        let model_id = stored["model"]
            .as_str()
            .ok_or_else(|| format!("免费供应商 `{name}` 包含无效模型条目"))?;
        if !seen.insert(model_id) {
            return Err(format!("免费供应商 `{name}` 重复声明模型 `{model_id}`"));
        }
        let model = preset
            .models
            .iter()
            .find(|candidate| candidate.id == model_id)
            .ok_or_else(|| {
                format!(
                    "模型 `{model_id}` 已不在免费供应商 `{name}` 的当前目录中，请重新验证"
                )
            })?;
        let expected = [
            ("tool_state", model.tool),
            ("vision_state", model.vision),
            ("json_schema_state", model.json_schema),
        ];
        for (field, capability) in expected {
            if stored[field] != serde_json::to_value(capability).expect("capability serializes") {
                return Err(format!(
                    "免费模型 `{model_id}` 的能力 `{field}` 与当前目录不一致，请重新验证"
                ));
            }
        }
        let expected_flags = [
            (
                "tool",
                matches!(model.tool, CapabilityState::Verified | CapabilityState::Declared),
            ),
            (
                "vision",
                matches!(
                    model.vision,
                    CapabilityState::Verified | CapabilityState::Declared
                ),
            ),
            (
                "json_schema",
                matches!(
                    model.json_schema,
                    CapabilityState::Verified | CapabilityState::Declared
                ),
            ),
        ];
        for (field, expected) in expected_flags {
            if stored[field].as_bool() != Some(expected) {
                return Err(format!(
                    "免费模型 `{model_id}` 的能力标记 `{field}` 与当前目录不一致，请重新验证"
                ));
            }
        }
        if stored["context_window"].as_u64() != Some(u64::from(model.context_window)) {
            return Err(format!(
                "免费模型 `{model_id}` 的上下文窗口与当前目录不一致，请重新验证"
            ));
        }
    }
    Ok(())
}

fn has_nonempty_message(document: &Value) -> bool {
    document
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| {
            choices.iter().any(|choice| {
                choice
                    .get("message")
                    .and_then(|message| message.get("content"))
                    .and_then(Value::as_str)
                    .is_some_and(|content| !content.trim().is_empty())
            })
        })
}

pub(crate) fn validate_chat_completion(
    preset: &FreeProviderPreset,
    model: &str,
    api_key: &str,
    egress: &EgressConfig,
    secrets: &SecretStore,
) -> Result<(), String> {
    let endpoint = ProviderEndpoint::try_new(preset.base_url)
        .map_err(|error| format!("免费供应商端点配置无效：{error}"))?;
    let url = endpoint.resolve(ProviderApi::ChatCompletions);
    let slot = SecretRef::new("provider_api_key");
    let mut provider = ProviderConfig::new("openai-compatible", endpoint);
    provider.auth = Some(slot.clone());
    let mut descriptor = HttpRequestDescriptor::new(HttpMethod::Post, &url);
    descriptor.auth = Some(
        Auth::header("authorization", slot)
            .map_err(|error| format!("免费供应商鉴权头配置无效：{error}"))?,
    );
    provider
        .authorize(&descriptor)
        .map_err(|error| format!("免费供应商出站请求被拒绝：{error}"))?;
    let http = token_station_cli::gateway::build_egress_agent(
        egress,
        Duration::from_secs(12),
        secrets,
    )?;
    let body = json!({
        "model": model,
        "messages": [{"role": "user", "content": "Reply OK"}],
        "max_tokens": 8,
        "temperature": 0,
        "stream": false
    })
    .to_string();
    let response = http
        .post(&descriptor.url)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("authorization", &format!("Bearer {}", api_key.trim()))
        .header(
            "user-agent",
            "token-station-desktop/free-provider-validation",
        )
        .send(&body)
        .map_err(|error| format!("免费模型验证网络失败：{error}"))?;
    let status = response.status().as_u16();
    if (300..400).contains(&status) {
        return Err(format!("免费模型验证拒绝上游重定向：HTTP {status}"));
    }
    if status >= 400 {
        return Err(match status {
            401 | 403 => "API Key 未通过供应商鉴权，请检查 Key、账号地区和权限".to_owned(),
            402 => "免费额度不可用，未保存供应商；请检查免费额度或更换平台".to_owned(),
            404 => "免费模型当前不可用，目录可能已变化".to_owned(),
            429 => "免费额度或速率限制已达到，请稍后重试".to_owned(),
            _ => format!("免费模型验证失败：HTTP {status}"),
        });
    }

    let mut bytes = Vec::new();
    response
        .into_body()
        .into_with_config()
        .limit(256 * 1024)
        .reader()
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取免费模型验证响应失败：{error}"))?;
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "供应商返回的验证响应不是有效 JSON".to_owned())?;
    if !has_nonempty_message(&document) {
        return Err("供应商未返回 OpenAI-compatible 的非空消息".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{has_nonempty_message, validate_stored_provider};
    use serde_json::json;

    #[test]
    fn rejects_structurally_empty_chat_responses() {
        assert!(!has_nonempty_message(&json!({"choices": [null]})));
        assert!(!has_nonempty_message(
            &json!({"choices": [{"message": {"content": "  "}}]})
        ));
        assert!(has_nonempty_message(
            &json!({"choices": [{"message": {"content": "OK"}}]})
        ));
    }

    #[test]
    fn stored_free_provider_must_match_the_current_catalog() {
        let valid = json!({
            "provider": "openai-compatible",
            "base_url": "https://api.groq.com/openai/v1",
            "access_tier": "free",
            "auth": {"slot": "provider_api_key", "keyring": true},
            "models": [{
                "model": "openai/gpt-oss-120b",
                "tool": false,
                "vision": false,
                "json_schema": false,
                "tool_state": "unknown",
                "vision_state": "unknown",
                "json_schema_state": "unknown",
                "context_window": 131072
            }]
        });
        validate_stored_provider("groq_free", &valid).unwrap();

        let mut stale = valid;
        stale["models"][0]["model"] = json!("paid-model");
        assert!(validate_stored_provider("groq_free", &stale)
            .unwrap_err()
            .contains("当前目录"));
    }
}
