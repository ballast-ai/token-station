use std::str::FromStr;

use json_five::rt::parser::{
    from_str as parse_json5_model, JSONKeyValuePair, JSONObjectContext, JSONTextContext, JSONValue,
    KeyValuePairContext,
};
use serde::de::{Error as DeError, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::{json, Value};
use toml_edit::{value as toml_value, Document, Item, Table, TableLike};
use yaml_edit::Document as YamlEditDocument;

use super::types::{ConfigPath, PatchKind, PatchOperation};

pub enum ConfigDocument {
    Json(Value),
    Json5(Json5Document),
    Toml(Box<Document>),
    Yaml(LosslessYamlDocument),
}

pub struct Json5Document {
    value: JSONValue,
    context: Option<JSONTextContext>,
}

pub struct LosslessYamlDocument {
    rendered: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocumentFormat {
    Json,
    Json5,
    Toml,
    Yaml,
}

pub fn parse_source_bytes(
    bytes: Option<&[u8]>,
    format: DocumentFormat,
    label: &str,
) -> Result<ConfigDocument, String> {
    match bytes {
        None => Ok(match format {
            DocumentFormat::Json => ConfigDocument::Json(json!({})),
            DocumentFormat::Json5 => empty_json5_document(),
            DocumentFormat::Toml => ConfigDocument::Toml(Box::new(Document::new())),
            DocumentFormat::Yaml => ConfigDocument::Yaml(LosslessYamlDocument {
                rendered: "{}\n".to_string(),
            }),
        }),
        Some(bytes) => {
            let rendered =
                std::str::from_utf8(bytes).map_err(|_| format!("{label} 不是有效 UTF-8 配置"))?;
            parse_rendered(rendered, format, label)
        }
    }
}

pub fn apply_patch(
    document: &mut ConfigDocument,
    operations: &[PatchOperation],
) -> Result<(), String> {
    for operation in operations {
        match document {
            ConfigDocument::Json(value) => apply_json_operation(value, operation)?,
            ConfigDocument::Json5(document) => apply_json5_operation(document, operation)?,
            ConfigDocument::Toml(value) => apply_toml_operation(value, operation)?,
            ConfigDocument::Yaml(document) => apply_yaml_operation(document, operation)?,
        }
    }
    Ok(())
}

/// Copy only declared owned subtrees from a baseline document into the
/// current document. JSON values and TOML `Item`s are cloned directly so
/// unrelated fields and TOML decoration outside the owned subtree survive.
pub fn project_owned_paths(
    current: &mut ConfigDocument,
    baseline: &ConfigDocument,
    owned_paths: &[ConfigPath],
) -> Result<(), String> {
    match (current, baseline) {
        (ConfigDocument::Json(current), ConfigDocument::Json(baseline)) => {
            for path in owned_paths {
                let value = json_value_at(baseline, path).cloned();
                let operation = PatchOperation {
                    operation: if value.is_some() {
                        PatchKind::Replace
                    } else {
                        PatchKind::Remove
                    },
                    path: path.clone(),
                    value,
                };
                apply_json_operation(current, &operation)?;
            }
            Ok(())
        }
        (ConfigDocument::Json5(current), ConfigDocument::Json5(baseline)) => {
            for path in owned_paths {
                let value = json5_value_at(&baseline.value, &path.segments)?.cloned();
                json5_set_value(&mut current.value, &path.segments, value)?;
            }
            Ok(())
        }
        (ConfigDocument::Toml(current), ConfigDocument::Toml(baseline)) => {
            for path in owned_paths {
                project_toml_path(current, baseline, path)?;
            }
            Ok(())
        }
        (ConfigDocument::Yaml(current), ConfigDocument::Yaml(baseline)) => {
            let baseline_semantic = strict_yaml_semantic(&baseline.rendered, "YAML 基线")?;
            for path in owned_paths {
                let value = json_value_at(&baseline_semantic, path).cloned();
                let operation = PatchOperation {
                    operation: if value.is_some() {
                        PatchKind::Replace
                    } else {
                        PatchKind::Remove
                    },
                    path: path.clone(),
                    value,
                };
                apply_yaml_operation(current, &operation)?;
            }
            Ok(())
        }
        _ => Err("当前配置与基线快照格式不一致".to_string()),
    }
}

fn empty_json5_document() -> ConfigDocument {
    ConfigDocument::Json5(Json5Document {
        value: JSONValue::JSONObject {
            key_value_pairs: Vec::new(),
            context: Some(JSONObjectContext {
                wsc: (String::new(),),
            }),
        },
        context: None,
    })
}

fn json5_semantic_key(key: &JSONValue) -> Result<String, String> {
    match key {
        JSONValue::Identifier(value) => Ok(value.clone()),
        JSONValue::DoubleQuotedString(value) => {
            json_five::from_str::<String>(&format!("\"{value}\""))
                .map_err(|_| "JSON5 属性名解码失败".to_string())
        }
        JSONValue::SingleQuotedString(value) => {
            json_five::from_str::<String>(&format!("'{value}'"))
                .map_err(|_| "JSON5 属性名解码失败".to_string())
        }
        _ => Err("JSON5 对象包含不合法属性名".to_string()),
    }
}

fn json5_property_index(
    pairs: &[JSONKeyValuePair],
    segment: &str,
) -> Result<Option<usize>, String> {
    let mut found = None;
    for (index, pair) in pairs.iter().enumerate() {
        if json5_semantic_key(&pair.key)? == segment {
            if found.is_some() {
                return Err(format!("JSON5 对象包含重复属性 '{segment}'"));
            }
            found = Some(index);
        }
    }
    Ok(found)
}

fn json5_value_at<'a>(
    value: &'a JSONValue,
    segments: &[String],
) -> Result<Option<&'a JSONValue>, String> {
    let Some((segment, remaining)) = segments.split_first() else {
        return Ok(Some(value));
    };
    let JSONValue::JSONObject {
        key_value_pairs, ..
    } = value
    else {
        return Ok(None);
    };
    let Some(index) = json5_property_index(key_value_pairs, segment)? else {
        return Ok(None);
    };
    json5_value_at(&key_value_pairs[index].value, remaining)
}

fn json5_from_serde(value: &Value) -> Result<JSONValue, String> {
    let rendered =
        serde_json::to_string(value).map_err(|_| "序列化 JSON5 patch value 失败".to_string())?;
    parse_json5_model(&rendered)
        .map(|document| document.value)
        .map_err(|_| "构造 JSON5 patch value 失败".to_string())
}

fn json5_key(segment: &str) -> Result<JSONValue, String> {
    json5_from_serde(&Value::String(segment.to_string()))
}

fn json5_empty_object() -> JSONValue {
    JSONValue::JSONObject {
        key_value_pairs: Vec::new(),
        context: Some(JSONObjectContext {
            wsc: (String::new(),),
        }),
    }
}

fn json5_append_property(
    pairs: &mut Vec<JSONKeyValuePair>,
    key: &str,
    value: JSONValue,
) -> Result<(), String> {
    let multiline = pairs.iter().any(|pair| {
        pair.context.as_ref().is_some_and(|context| {
            context.wsc.2.contains('\n')
                || context
                    .wsc
                    .3
                    .as_ref()
                    .is_some_and(|value| value.contains('\n'))
        })
    });
    if let Some(last) = pairs.last_mut() {
        match &mut last.context {
            Some(context) if context.wsc.3.is_none() => {
                let trailing = std::mem::take(&mut context.wsc.2);
                context.wsc.3 = Some(if trailing.is_empty() {
                    if multiline {
                        "\n  ".to_string()
                    } else {
                        " ".to_string()
                    }
                } else {
                    trailing
                });
            }
            None => {
                last.context = Some(KeyValuePairContext {
                    wsc: (
                        String::new(),
                        " ".to_string(),
                        String::new(),
                        Some(if multiline {
                            "\n  ".to_string()
                        } else {
                            " ".to_string()
                        }),
                    ),
                });
            }
            _ => {}
        }
    }
    pairs.push(JSONKeyValuePair {
        key: json5_key(key)?,
        value,
        context: Some(KeyValuePairContext {
            wsc: (
                String::new(),
                " ".to_string(),
                if multiline {
                    "\n".to_string()
                } else {
                    String::new()
                },
                None,
            ),
        }),
    });
    Ok(())
}

fn json5_set_value(
    current: &mut JSONValue,
    segments: &[String],
    replacement: Option<JSONValue>,
) -> Result<(), String> {
    let Some((segment, remaining)) = segments.split_first() else {
        return Err("JSON5 patch 路径不能为空".to_string());
    };
    let JSONValue::JSONObject {
        key_value_pairs, ..
    } = current
    else {
        return Err("JSON5 patch 路径的父级不是对象".to_string());
    };
    let index = json5_property_index(key_value_pairs, segment)?;
    if remaining.is_empty() {
        match (index, replacement) {
            (Some(index), Some(value)) => key_value_pairs[index].value = value,
            (Some(index), None) => {
                key_value_pairs.remove(index);
            }
            (None, Some(value)) => json5_append_property(key_value_pairs, segment, value)?,
            (None, None) => {}
        }
        return Ok(());
    }
    let child_index = if let Some(index) = index {
        index
    } else if replacement.is_none() {
        return Ok(());
    } else {
        json5_append_property(key_value_pairs, segment, json5_empty_object())?;
        key_value_pairs.len() - 1
    };
    json5_set_value(
        &mut key_value_pairs[child_index].value,
        remaining,
        replacement,
    )
}

fn apply_json5_operation(
    document: &mut Json5Document,
    operation: &PatchOperation,
) -> Result<(), String> {
    match operation.operation {
        PatchKind::Add | PatchKind::Replace => {
            let value = operation
                .value
                .as_ref()
                .ok_or_else(|| format!("配置路径 '{}' 缺少写入值", operation.path))?;
            json5_set_value(
                &mut document.value,
                &operation.path.segments,
                Some(json5_from_serde(value)?),
            )
        }
        PatchKind::Remove => json5_set_value(&mut document.value, &operation.path.segments, None),
        PatchKind::Test => {
            let actual = json5_value_at(&document.value, &operation.path.segments)?
                .map(json5_value_as_serde)
                .transpose()?;
            if actual.as_ref() == operation.value.as_ref() {
                Ok(())
            } else {
                Err(format!("配置路径 '{}' 的前置值已变化", operation.path))
            }
        }
    }
}

fn json5_value_as_serde(value: &JSONValue) -> Result<Value, String> {
    json_five::from_str(&value.to_string()).map_err(|_| "JSON5 语义转换失败".to_string())
}

fn json_value_at<'a>(root: &'a Value, path: &ConfigPath) -> Option<&'a Value> {
    let mut current = root;
    for segment in &path.segments {
        current = current.as_object()?.get(segment)?;
    }
    Some(current)
}

fn project_toml_path(
    current: &mut Document,
    baseline: &Document,
    path: &ConfigPath,
) -> Result<(), String> {
    let (last, parents) = split_path(path)?;
    let mut baseline_table: Option<&dyn TableLike> = Some(baseline.as_table());
    for segment in parents {
        baseline_table = baseline_table
            .and_then(|table| table.get(segment))
            .and_then(Item::as_table_like);
    }
    let baseline_item = baseline_table.and_then(|table| table.get(last)).cloned();
    let mut current_table: &mut dyn TableLike = current.as_table_mut();
    if let Some(item) = baseline_item {
        for segment in parents {
            if !current_table.contains_key(segment) {
                current_table.insert(segment, Item::Table(Table::new()));
            }
            current_table = current_table
                .get_mut(segment)
                .and_then(Item::as_table_like_mut)
                .ok_or_else(|| format!("配置路径 '{path}' 的父级不是 TOML 表"))?;
        }
        current_table.insert(last, item);
        return Ok(());
    }
    for segment in parents {
        let Some(next) = current_table.get_mut(segment) else {
            return Ok(());
        };
        current_table = next
            .as_table_like_mut()
            .ok_or_else(|| format!("配置路径 '{path}' 的父级不是 TOML 表"))?;
    }
    current_table.remove(last);
    Ok(())
}

fn apply_json_operation(root: &mut Value, operation: &PatchOperation) -> Result<(), String> {
    let (last, parents) = split_path(&operation.path)?;
    let mut cursor = root;
    match operation.operation {
        PatchKind::Add | PatchKind::Replace => {
            for segment in parents {
                let object = cursor
                    .as_object_mut()
                    .ok_or_else(|| format!("配置路径 '{}' 的父级不是对象", operation.path))?;
                cursor = object.entry(segment.clone()).or_insert_with(|| json!({}));
            }
            let object = cursor
                .as_object_mut()
                .ok_or_else(|| format!("配置路径 '{}' 的父级不是对象", operation.path))?;
            let value = operation
                .value
                .clone()
                .ok_or_else(|| format!("配置路径 '{}' 缺少写入值", operation.path))?;
            object.insert(last.clone(), value);
        }
        PatchKind::Remove => {
            for segment in parents {
                let Some(object) = cursor.as_object_mut() else {
                    return Err(format!("配置路径 '{}' 的父级不是对象", operation.path));
                };
                let Some(next) = object.get_mut(segment) else {
                    return Ok(());
                };
                cursor = next;
            }
            let object = cursor
                .as_object_mut()
                .ok_or_else(|| format!("配置路径 '{}' 的父级不是对象", operation.path))?;
            object.remove(last);
        }
        PatchKind::Test => {
            for segment in parents {
                let object = cursor
                    .as_object_mut()
                    .ok_or_else(|| format!("配置路径 '{}' 的父级不是对象", operation.path))?;
                let Some(next) = object.get_mut(segment) else {
                    return Err(format!("配置路径 '{}' 的前置值已变化", operation.path));
                };
                cursor = next;
            }
            let object = cursor
                .as_object_mut()
                .ok_or_else(|| format!("配置路径 '{}' 的父级不是对象", operation.path))?;
            if object.get(last) != operation.value.as_ref() {
                return Err(format!("配置路径 '{}' 的前置值已变化", operation.path));
            }
        }
    }
    Ok(())
}

fn apply_toml_operation(document: &mut Document, operation: &PatchOperation) -> Result<(), String> {
    let (last, parents) = split_path(&operation.path)?;
    let mut table: &mut dyn TableLike = document.as_table_mut();
    match operation.operation {
        PatchKind::Add | PatchKind::Replace => {
            for segment in parents {
                if !table.contains_key(segment) {
                    table.insert(segment, Item::Table(Table::new()));
                }
                table = table
                    .get_mut(segment)
                    .and_then(Item::as_table_like_mut)
                    .ok_or_else(|| format!("配置路径 '{}' 的父级不是 TOML 表", operation.path))?;
            }
            let item = json_scalar_to_toml(
                operation
                    .value
                    .as_ref()
                    .ok_or_else(|| format!("配置路径 '{}' 缺少写入值", operation.path))?,
                &operation.path,
            )?;
            table.insert(last, item);
        }
        PatchKind::Remove => {
            for segment in parents {
                let Some(next) = table.get_mut(segment) else {
                    return Ok(());
                };
                table = next
                    .as_table_like_mut()
                    .ok_or_else(|| format!("配置路径 '{}' 的父级不是 TOML 表", operation.path))?;
            }
            table.remove(last);
        }
        PatchKind::Test => {
            return Err("TOML patch 暂不支持 test 操作".to_string());
        }
    }
    Ok(())
}

fn apply_yaml_operation(
    document: &mut LosslessYamlDocument,
    operation: &PatchOperation,
) -> Result<(), String> {
    split_path(&operation.path)?;
    let semantic = strict_yaml_semantic(&document.rendered, "YAML patch")?;
    match operation.operation {
        PatchKind::Add | PatchKind::Replace => {
            let value = operation
                .value
                .as_ref()
                .ok_or_else(|| format!("配置路径 '{}' 缺少写入值", operation.path))?;
            if json_value_at(&semantic, &operation.path).is_none() {
                let rendered =
                    insert_missing_yaml_scalar(&document.rendered, &operation.path, value)?;
                validate_yaml_rendered(&rendered, "YAML patch")?;
                document.rendered = rendered;
                return Ok(());
            }
            let rendered =
                replace_existing_yaml_scalar(&document.rendered, &operation.path, value)?;
            validate_yaml_rendered(&rendered, "YAML patch")?;
            document.rendered = rendered;
            return Ok(());
        }
        PatchKind::Remove => {
            if json_value_at(&semantic, &operation.path).is_some() {
                let rendered = remove_existing_yaml_scalar(&document.rendered, &operation.path)?;
                validate_yaml_rendered(&rendered, "YAML patch")?;
                document.rendered = rendered;
            }
            return Ok(());
        }
        PatchKind::Test => {
            let semantic = strict_yaml_semantic(&document.rendered, "YAML test")?;
            if json_value_at(&semantic, &operation.path) != operation.value.as_ref() {
                return Err(format!("配置路径 '{}' 的前置值已变化", operation.path));
            }
        }
    }
    strict_yaml_semantic(&document.rendered, "YAML patch")?;
    Ok(())
}

fn insert_missing_yaml_scalar(
    rendered: &str,
    path: &ConfigPath,
    value: &Value,
) -> Result<String, String> {
    let [root, child] = path.segments.as_slice() else {
        return Err(format!(
            "配置路径 '{}' 暂不支持安全地新增 YAML 嵌套字段",
            path
        ));
    };
    if !is_plain_yaml_key(root) || !is_plain_yaml_key(child) {
        return Err(format!("配置路径 '{}' 包含不安全的 YAML 键", path));
    }
    let scalar = yaml_scalar(value, path)?;
    let root_prefix = format!("{root}:");
    let mut root_line_end = None;
    let mut insertion = rendered.len();

    for (offset, line) in rendered.split_inclusive('\n').scan(0usize, |cursor, line| {
        let offset = *cursor;
        *cursor += line.len();
        Some((offset, line))
    }) {
        let without_newline = line.trim_end_matches(['\r', '\n']);
        if root_line_end.is_none() {
            if let Some(rest) = without_newline.strip_prefix(&root_prefix) {
                let suffix = rest.trim();
                if !suffix.is_empty() && !suffix.starts_with('#') {
                    return Err(format!("配置路径 '{}' 的父级必须使用 YAML 块映射", path));
                }
                root_line_end = Some(offset + line.len());
            }
            continue;
        }

        let trimmed = without_newline.trim_start();
        let indent = without_newline.len() - trimmed.len();
        if !trimmed.is_empty() && indent == 0 {
            insertion = offset;
            break;
        }
    }

    let mut updated = rendered.to_string();
    if root_line_end.is_some() {
        let line = format!("  {child}: {scalar}\n");
        updated.insert_str(insertion, &line);
    } else {
        // `parse_source_bytes(None, Yaml, ..)` represents an empty mapping as
        // the valid flow document `{}`. Appending a block mapping after it
        // would create two root nodes, so replace only this semantic-empty
        // representation. A non-empty document is always preserved.
        if updated.trim() == "{}" {
            updated.clear();
        }
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(&format!("{root}:\n  {child}: {scalar}\n"));
    }
    Ok(updated)
}

fn is_plain_yaml_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn yaml_scalar(value: &Value, path: &ConfigPath) -> Result<String, String> {
    if !matches!(value, Value::Null | Value::String(_) | Value::Bool(_)) && value.as_i64().is_none()
    {
        return Err(format!("配置路径 '{}' 只允许 YAML 标量", path));
    }
    let rendered = serde_norway::to_string(value)
        .map_err(|_| format!("配置路径 '{}' 的 YAML 标量序列化失败", path))?;
    let scalar = rendered.strip_suffix('\n').unwrap_or(&rendered);
    if scalar.contains(['\r', '\n']) {
        return Err(format!("配置路径 '{}' 只允许单行 YAML 标量", path));
    }
    Ok(scalar.to_string())
}

fn replace_existing_yaml_scalar(
    rendered: &str,
    path: &ConfigPath,
    value: &Value,
) -> Result<String, String> {
    let [root, child] = path.segments.as_slice() else {
        return Err(format!(
            "配置路径 '{}' 暂不支持安全地替换 YAML 嵌套字段",
            path
        ));
    };
    if !is_plain_yaml_key(root) || !is_plain_yaml_key(child) {
        return Err(format!("配置路径 '{}' 包含不安全的 YAML 键", path));
    }
    let scalar = yaml_scalar(value, path)?;
    let root_prefix = format!("{root}:");
    let child_prefix = format!("{child}:");
    let mut inside_root = false;
    let mut direct_indent = None;
    let mut replacement = None;

    for (offset, line) in rendered.split_inclusive('\n').scan(0usize, |cursor, line| {
        let offset = *cursor;
        *cursor += line.len();
        Some((offset, line))
    }) {
        let without_newline = line.trim_end_matches(['\r', '\n']);
        let trimmed = without_newline.trim_start_matches(' ');
        let indent = without_newline.len() - trimmed.len();

        if !inside_root {
            if indent == 0 {
                if let Some(rest) = trimmed.strip_prefix(&root_prefix) {
                    let suffix = rest.trim();
                    if !suffix.is_empty() && !suffix.starts_with('#') {
                        return Err(format!("配置路径 '{}' 的父级必须使用 YAML 块映射", path));
                    }
                    inside_root = true;
                }
            }
            continue;
        }

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if indent == 0 {
            break;
        }
        let expected_indent = *direct_indent.get_or_insert(indent);
        if indent != expected_indent {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix(&child_prefix) else {
            continue;
        };
        if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
            continue;
        }
        let value_start = offset + indent + child_prefix.len();
        let comment = yaml_inline_comment_start(rest);
        let value_end = comment.map_or(offset + without_newline.len(), |index| value_start + index);
        let existing = &rendered[value_start..value_end];
        if matches!(existing.trim_start().chars().next(), Some('|' | '>')) {
            return Err(format!("配置路径 '{}' 不能替换 YAML 块标量", path));
        }
        let replacement_text = if comment.is_some() {
            format!(" {scalar} ")
        } else {
            format!(" {scalar}")
        };
        replacement = Some((value_start, value_end, replacement_text));
        break;
    }

    let Some((start, end, replacement_text)) = replacement else {
        return Err(format!("配置路径 '{}' 不是可安全替换的 YAML 块字段", path));
    };
    let mut updated = rendered.to_string();
    updated.replace_range(start..end, &replacement_text);
    Ok(updated)
}

fn yaml_inline_comment_start(value_and_comment: &str) -> Option<usize> {
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;
    let mut previous_whitespace = true;
    let mut characters = value_and_comment.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if double_quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                double_quoted = false;
            }
            continue;
        }
        if single_quoted {
            if character == '\'' {
                if characters.peek().is_some_and(|(_, next)| *next == '\'') {
                    characters.next();
                } else {
                    single_quoted = false;
                }
            }
            continue;
        }
        match character {
            '"' => double_quoted = true,
            '\'' => single_quoted = true,
            '#' if previous_whitespace => return Some(index),
            _ => {}
        }
        previous_whitespace = character.is_whitespace();
    }
    None
}

fn remove_existing_yaml_scalar(rendered: &str, path: &ConfigPath) -> Result<String, String> {
    let [root, child] = path.segments.as_slice() else {
        return Err(format!(
            "配置路径 '{}' 暂不支持安全地移除 YAML 嵌套字段",
            path
        ));
    };
    if !is_plain_yaml_key(root) || !is_plain_yaml_key(child) {
        return Err(format!("配置路径 '{}' 包含不安全的 YAML 键", path));
    }
    let root_prefix = format!("{root}:");
    let child_prefix = format!("{child}:");
    let mut inside_root = false;
    let mut direct_indent = None;
    let mut removal = None;

    for (offset, line) in rendered.split_inclusive('\n').scan(0usize, |cursor, line| {
        let offset = *cursor;
        *cursor += line.len();
        Some((offset, line))
    }) {
        let without_newline = line.trim_end_matches(['\r', '\n']);
        let trimmed = without_newline.trim_start_matches(' ');
        let indent = without_newline.len() - trimmed.len();
        if !inside_root {
            if indent == 0 && trimmed.starts_with(&root_prefix) {
                inside_root = true;
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if indent == 0 {
            break;
        }
        let expected_indent = *direct_indent.get_or_insert(indent);
        if indent != expected_indent {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix(&child_prefix) else {
            continue;
        };
        if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
            continue;
        }
        if matches!(rest.trim_start().chars().next(), Some('|' | '>')) {
            return Err(format!("配置路径 '{}' 不能移除 YAML 块标量", path));
        }
        removal = Some((offset, offset + line.len()));
        break;
    }

    let Some((start, end)) = removal else {
        return Err(format!("配置路径 '{}' 不是可安全移除的 YAML 块字段", path));
    };
    let mut updated = rendered.to_string();
    updated.replace_range(start..end, "");
    Ok(updated)
}

fn validate_yaml_rendered(rendered: &str, label: &str) -> Result<(), String> {
    let semantic = strict_yaml_semantic(rendered, label)?;
    if !semantic.is_object() {
        return Err(format!("写入前复验 {label} 顶层不是对象"));
    }
    let document = YamlEditDocument::from_str(rendered)
        .map_err(|_| format!("写入前复验 {label} YAML CST 解析失败"))?;
    if document.as_mapping().is_none() {
        return Err(format!("写入前复验 {label} 顶层不是对象"));
    }
    Ok(())
}

struct StrictYamlValue(Value);

impl<'de> Deserialize<'de> for StrictYamlValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StrictYamlVisitor;

        impl<'de> Visitor<'de> for StrictYamlVisitor {
            type Value = StrictYamlValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a YAML value without duplicate or merge keys")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
                Ok(StrictYamlValue(Value::Bool(value)))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(StrictYamlValue(Value::Number(value.into())))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(StrictYamlValue(Value::Number(value.into())))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                serde_json::Number::from_f64(value)
                    .map(Value::Number)
                    .map(StrictYamlValue)
                    .ok_or_else(|| E::custom("YAML_FLOAT_NOT_FINITE"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(StrictYamlValue(Value::String(value.to_string())))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(StrictYamlValue(Value::String(value)))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(StrictYamlValue(Value::Null))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(StrictYamlValue(Value::Null))
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<StrictYamlValue>()? {
                    values.push(value.0);
                }
                Ok(StrictYamlValue(Value::Array(values)))
            }

            fn visit_map<A>(self, mut mapping: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = serde_json::Map::new();
                while let Some(key) = mapping.next_key::<String>()? {
                    if key == "<<" {
                        return Err(A::Error::custom("YAML_MERGE_KEY_FORBIDDEN"));
                    }
                    if values.contains_key(&key) {
                        return Err(A::Error::custom("YAML_DUPLICATE_KEY"));
                    }
                    let value = mapping.next_value::<StrictYamlValue>()?;
                    values.insert(key, value.0);
                }
                Ok(StrictYamlValue(Value::Object(values)))
            }
        }

        deserializer.deserialize_any(StrictYamlVisitor)
    }
}

fn strict_yaml_semantic(rendered: &str, label: &str) -> Result<Value, String> {
    let mut documents = serde_norway::Deserializer::from_str(rendered);
    let Some(first) = documents.next() else {
        return Ok(json!({}));
    };
    let value = StrictYamlValue::deserialize(first).map_err(|error| {
        let summary = error.to_string();
        if summary.contains("YAML_DUPLICATE_KEY") {
            format!("写入前复验 {label} YAML 包含重复属性")
        } else if summary.contains("YAML_MERGE_KEY_FORBIDDEN") {
            format!("写入前复验 {label} YAML 不允许 merge key")
        } else {
            error.location().map_or_else(
                || format!("写入前复验 {label} YAML 失败（位置未知）"),
                |location| {
                    format!(
                        "写入前复验 {label} YAML 失败（行 {} 列 {}）",
                        location.line(),
                        location.column()
                    )
                },
            )
        }
    })?;
    if documents.next().is_some() {
        return Err(format!("写入前复验 {label} YAML 必须是单文档"));
    }
    Ok(value.0)
}

fn json_scalar_to_toml(value: &Value, path: &ConfigPath) -> Result<Item, String> {
    match value {
        Value::String(value) => Ok(toml_value(value.clone())),
        Value::Bool(value) => Ok(toml_value(*value)),
        Value::Number(value) => value
            .as_i64()
            .map(toml_value)
            .ok_or_else(|| format!("配置路径 '{path}' 不是受支持的 TOML 整数")),
        _ => Err(format!("配置路径 '{path}' 只允许 TOML 标量")),
    }
}

fn split_path(path: &ConfigPath) -> Result<(&String, &[String]), String> {
    let (last, parents) = path
        .segments
        .split_last()
        .ok_or_else(|| "配置 patch 路径不能为空".to_string())?;
    if path.segments.iter().any(|segment| {
        segment.is_empty()
            || matches!(segment.as_str(), "." | "..")
            || segment.contains(['/', '\\', '\0'])
    }) {
        return Err(format!("配置 patch 路径 '{path}' 不合法"));
    }
    Ok((last, parents))
}

pub fn render_document(document: &ConfigDocument, label: &str) -> Result<String, String> {
    match document {
        ConfigDocument::Json(value) => serde_json::to_string_pretty(value)
            .map_err(|error| format!("序列化 {label} 失败：{error}")),
        ConfigDocument::Json5(document) => Ok(document.context.as_ref().map_or_else(
            || document.value.to_string(),
            |context| format!("{}{}{}", context.wsc.0, document.value, context.wsc.1),
        )),
        ConfigDocument::Toml(document) => Ok(document.to_string()),
        ConfigDocument::Yaml(document) => Ok(document.rendered.clone()),
    }
}

pub fn semantic_json(document: &ConfigDocument) -> Result<Value, String> {
    match document {
        ConfigDocument::Json(value) => Ok(value.clone()),
        ConfigDocument::Json5(document) => json5_value_as_serde(&document.value),
        ConfigDocument::Toml(_) => {
            let rendered = render_document(document, "TOML semantic projection")?;
            let value: toml::Value =
                toml::from_str(&rendered).map_err(|_| "TOML 语义解析失败".to_string())?;
            serde_json::to_value(value).map_err(|_| "TOML 语义转换失败".to_string())
        }
        ConfigDocument::Yaml(document) => {
            strict_yaml_semantic(&document.rendered, "YAML semantic projection")
        }
    }
}

pub fn parse_rendered(
    rendered: &str,
    format: DocumentFormat,
    label: &str,
) -> Result<ConfigDocument, String> {
    match format {
        DocumentFormat::Json => {
            let value: Value = serde_json::from_str(rendered).map_err(|error| {
                format!(
                    "写入前复验 {label} JSON 失败（行 {} 列 {}）",
                    error.line(),
                    error.column()
                )
            })?;
            if value.is_object() {
                Ok(ConfigDocument::Json(value))
            } else {
                Err(format!("写入前复验 {label} 顶层不是对象"))
            }
        }
        DocumentFormat::Json5 => {
            let document = parse_json5_model(rendered).map_err(|error| {
                format!(
                    "写入前复验 {label} JSON5 失败（行 {} 列 {}）",
                    error.lineno, error.colno
                )
            })?;
            if !matches!(document.value, JSONValue::JSONObject { .. }) {
                return Err(format!("写入前复验 {label} 顶层不是对象"));
            }
            validate_json5_value(&document.value)?;
            json_five::from_str::<Value>(rendered)
                .map_err(|_| format!("写入前复验 {label} JSON5 语义解析失败"))?;
            Ok(ConfigDocument::Json5(Json5Document {
                value: document.value,
                context: document.context,
            }))
        }
        DocumentFormat::Toml => Document::from_str(rendered)
            .map(Box::new)
            .map(ConfigDocument::Toml)
            .map_err(|error| {
                format!(
                    "写入前复验 {label} TOML 失败（{}）",
                    toml_error_location(&error)
                )
            }),
        DocumentFormat::Yaml => {
            let semantic = strict_yaml_semantic(rendered, label)?;
            if !semantic.is_object() {
                return Err(format!("写入前复验 {label} 顶层不是对象"));
            }
            validate_yaml_rendered(rendered, label)?;
            Ok(ConfigDocument::Yaml(LosslessYamlDocument {
                rendered: rendered.to_string(),
            }))
        }
    }
}

fn validate_json5_value(value: &JSONValue) -> Result<(), String> {
    match value {
        JSONValue::JSONObject {
            key_value_pairs, ..
        } => {
            let mut names = std::collections::BTreeSet::new();
            for pair in key_value_pairs {
                let name = json5_semantic_key(&pair.key)?;
                if !names.insert(name.clone()) {
                    return Err(format!("JSON5 对象包含重复属性 '{name}'"));
                }
                validate_json5_value(&pair.value)?;
            }
        }
        JSONValue::JSONArray { values, .. } => {
            for value in values {
                validate_json5_value(&value.value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn toml_error_location(error: &toml_edit::TomlError) -> String {
    error.span().map_or_else(
        || "位置未知".to_string(),
        |span| format!("字节偏移 {}", span.start),
    )
}

impl std::fmt::Display for ConfigPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "/{}", self.segments.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operation(kind: PatchKind, segments: &[&str], value: Option<Value>) -> PatchOperation {
        PatchOperation {
            operation: kind,
            path: ConfigPath {
                segments: segments.iter().map(|value| (*value).to_string()).collect(),
            },
            value,
        }
    }

    fn remove(segments: &[&str]) -> PatchOperation {
        PatchOperation {
            operation: PatchKind::Remove,
            path: ConfigPath {
                segments: segments.iter().map(|value| (*value).to_string()).collect(),
            },
            value: None,
        }
    }

    #[test]
    fn config_remove_never_creates_missing_json_or_toml_parents() {
        let mut json = ConfigDocument::Json(json!({ "keep": true }));
        apply_patch(&mut json, &[remove(&["provider", "tokenstation"])]).unwrap();
        assert_eq!(
            render_document(&json, "fixture").unwrap(),
            "{\n  \"keep\": true\n}"
        );

        let mut toml = ConfigDocument::Toml(Box::new(Document::new()));
        apply_patch(&mut toml, &[remove(&["model_providers", "tokenstation"])]).unwrap();
        assert!(render_document(&toml, "fixture").unwrap().is_empty());
    }

    #[test]
    fn config_parse_errors_never_echo_the_source_line() {
        let marker = "sk-sensitive-marker-must-not-leak";
        let json = format!("{{\"apiKey\":\"{marker}\",}}");
        let json_error = parse_rendered(&json, DocumentFormat::Json, "fixture")
            .err()
            .expect("invalid JSON is rejected");
        assert!(!json_error.contains(marker), "{json_error}");

        let toml = format!("api_key = \"{marker}\"\nbroken = [\n");
        let toml_error = parse_rendered(&toml, DocumentFormat::Toml, "fixture")
            .err()
            .expect("invalid TOML is rejected");
        assert!(!toml_error.contains(marker), "{toml_error}");
        assert!(!toml_error.contains("broken ="), "{toml_error}");
    }

    #[test]
    fn config_patch_format_matrix_covers_tests_scalars_invalid_paths_and_projection_mismatch() {
        let mut json = ConfigDocument::Json(json!({"root": {"value": 1}, "scalar": true}));
        apply_patch(
            &mut json,
            &[
                operation(PatchKind::Test, &["root", "value"], Some(json!(1))),
                operation(PatchKind::Add, &["root", "text"], Some(json!("ok"))),
                operation(PatchKind::Remove, &["root", "missing"], None),
            ],
        )
        .unwrap();
        assert!(apply_patch(
            &mut json,
            &[operation(
                PatchKind::Test,
                &["root", "value"],
                Some(json!(2))
            )]
        )
        .unwrap_err()
        .contains("前置值"));
        assert!(apply_patch(
            &mut json,
            &[operation(
                PatchKind::Replace,
                &["scalar", "child"],
                Some(json!(1))
            )]
        )
        .is_err());
        for segments in [Vec::<&str>::new(), vec![".."], vec!["bad/key"]] {
            assert!(
                apply_patch(&mut json, &[operation(PatchKind::Remove, &segments, None)]).is_err()
            );
        }
        assert!(apply_patch(
            &mut json,
            &[operation(PatchKind::Add, &["missing-value"], None)]
        )
        .is_err());

        let mut json5 =
            parse_rendered("{ root: { value: 1 } }", DocumentFormat::Json5, "fixture").unwrap();
        apply_patch(
            &mut json5,
            &[
                operation(PatchKind::Test, &["root", "value"], Some(json!(1))),
                operation(PatchKind::Add, &["root", "added"], Some(json!(true))),
                operation(PatchKind::Remove, &["root", "value"], None),
            ],
        )
        .unwrap();
        assert_eq!(semantic_json(&json5).unwrap()["root"]["added"], true);
        assert!(apply_patch(
            &mut json5,
            &[operation(
                PatchKind::Test,
                &["root", "added"],
                Some(json!(false))
            )]
        )
        .is_err());
        let mut scalar_parent =
            parse_rendered("{ root: true }", DocumentFormat::Json5, "fixture").unwrap();
        assert!(apply_patch(
            &mut scalar_parent,
            &[operation(
                PatchKind::Add,
                &["root", "child"],
                Some(json!(1))
            )]
        )
        .is_err());

        let mut toml = parse_rendered("root = 1\n", DocumentFormat::Toml, "fixture").unwrap();
        assert!(apply_patch(
            &mut toml,
            &[operation(PatchKind::Test, &["root"], Some(json!(1)))]
        )
        .unwrap_err()
        .contains("不支持 test"));
        for value in [json!("text"), json!(true), json!(42)] {
            apply_patch(
                &mut toml,
                &[operation(
                    PatchKind::Replace,
                    &["table", "field"],
                    Some(value),
                )],
            )
            .unwrap();
        }
        for value in [json!(1.5), json!([1]), json!({"a": 1})] {
            assert!(apply_patch(
                &mut toml,
                &[operation(PatchKind::Replace, &["bad"], Some(value))]
            )
            .is_err());
        }

        let mut yaml = parse_rendered(
            "model:\n  existing: old\nnext: true\n",
            DocumentFormat::Yaml,
            "fixture",
        )
        .unwrap();
        apply_patch(
            &mut yaml,
            &[
                operation(
                    PatchKind::Replace,
                    &["model", "existing"],
                    Some(json!("new")),
                ),
                operation(PatchKind::Add, &["model", "enabled"], Some(json!(true))),
                operation(PatchKind::Add, &["model", "count"], Some(json!(7))),
                operation(PatchKind::Test, &["model", "count"], Some(json!(7))),
            ],
        )
        .unwrap();
        let rendered = render_document(&yaml, "fixture").unwrap();
        assert!(rendered.contains("  enabled: true"), "{rendered}");
        assert!(rendered.contains("  count: 7"), "{rendered}");
        assert!(apply_patch(
            &mut yaml,
            &[operation(
                PatchKind::Test,
                &["model", "count"],
                Some(json!(8))
            )]
        )
        .is_err());
        assert!(apply_patch(
            &mut yaml,
            &[operation(
                PatchKind::Add,
                &["model", "float"],
                Some(json!(1.5))
            )]
        )
        .is_err());
        assert!(apply_patch(
            &mut yaml,
            &[operation(
                PatchKind::Add,
                &["model", "nested", "field"],
                Some(json!(1))
            )]
        )
        .is_err());
        assert!(apply_patch(
            &mut yaml,
            &[operation(PatchKind::Add, &["model", ":"], Some(json!(1)))]
        )
        .is_err());
        let mut inline =
            parse_rendered("model: {keep: true}\n", DocumentFormat::Yaml, "fixture").unwrap();
        assert!(apply_patch(
            &mut inline,
            &[operation(
                PatchKind::Add,
                &["model", "added"],
                Some(json!(1))
            )]
        )
        .is_err());

        let baseline = parse_source_bytes(None, DocumentFormat::Toml, "fixture").unwrap();
        assert!(project_owned_paths(
            &mut json,
            &baseline,
            &[ConfigPath {
                segments: vec!["root".to_string()]
            }]
        )
        .unwrap_err()
        .contains("格式不一致"));
        assert!(parse_source_bytes(Some(&[0xff]), DocumentFormat::Json, "fixture").is_err());
        for (source, format) in [
            ("[]", DocumentFormat::Json),
            ("[]", DocumentFormat::Json5),
            ("- item\n", DocumentFormat::Yaml),
        ] {
            assert!(parse_rendered(source, format, "fixture").is_err());
        }
        assert_eq!(semantic_json(&toml).unwrap()["root"], 1);
    }

    #[test]
    fn config_json5_projection_preserves_comments_unknown_fields_and_rejects_duplicates() {
        let source = r#"// root comment
{
  channels: { keep: true }, // unknown subtree
  models: {
    providers: {
      existing: { baseUrl: 'https://keep.invalid/v1' },
    },
  },
}
"#;
        let mut document = parse_rendered(source, DocumentFormat::Json5, "OpenClaw").unwrap();
        let operation = PatchOperation {
            operation: PatchKind::Replace,
            path: ConfigPath {
                segments: vec![
                    "models".to_string(),
                    "providers".to_string(),
                    "tokenstation".to_string(),
                ],
            },
            value: Some(json!({
                "baseUrl": "http://127.0.0.1:8787/v1",
                "api": "openai-completions"
            })),
        };
        apply_patch(&mut document, &[operation]).unwrap();
        let rendered = render_document(&document, "OpenClaw").unwrap();
        assert!(rendered.contains("// root comment"), "{rendered}");
        assert!(rendered.contains("// unknown subtree"), "{rendered}");
        assert!(rendered.contains("https://keep.invalid/v1"), "{rendered}");
        let semantic = semantic_json(&document).unwrap();
        assert_eq!(semantic["channels"]["keep"], true);
        assert_eq!(
            semantic["models"]["providers"]["tokenstation"]["api"],
            "openai-completions"
        );

        let duplicate = "{ models: {}, 'models': {} }";
        let error = parse_rendered(duplicate, DocumentFormat::Json5, "OpenClaw")
            .err()
            .expect("duplicate JSON5 keys fail closed");
        assert!(error.contains("重复属性"), "{error}");
    }

    #[test]
    fn config_yaml_projection_is_lossless_and_rejects_ambiguous_input() {
        let source = r#"# root comment
_config_version: 33
model:
  default: old-model # keep model comment
  provider: old-provider
  unknown: keep-me
display:
  skin: default # keep unrelated comment
"#;
        let mut document = parse_rendered(source, DocumentFormat::Yaml, "Hermes").unwrap();
        let operations = [
            PatchOperation {
                operation: PatchKind::Replace,
                path: ConfigPath {
                    segments: vec!["model".to_string(), "default".to_string()],
                },
                value: Some(json!("auto")),
            },
            PatchOperation {
                operation: PatchKind::Replace,
                path: ConfigPath {
                    segments: vec!["model".to_string(), "provider".to_string()],
                },
                value: Some(json!("custom")),
            },
        ];
        apply_patch(&mut document, &operations).unwrap();
        let rendered = render_document(&document, "Hermes").unwrap();
        assert!(rendered.starts_with("# root comment\n"), "{rendered}");
        assert!(rendered.contains("# keep model comment"), "{rendered}");
        assert!(rendered.contains("unknown: keep-me"), "{rendered}");
        assert!(
            rendered.contains("skin: default # keep unrelated comment"),
            "{rendered}"
        );
        let semantic = semantic_json(&document).unwrap();
        assert_eq!(semantic["model"]["default"], "auto");
        assert_eq!(semantic["model"]["provider"], "custom");

        let duplicate = "model:\n  provider: custom\n  provider: openrouter\n";
        let error = parse_rendered(duplicate, DocumentFormat::Yaml, "Hermes")
            .err()
            .expect("duplicate YAML keys fail closed");
        assert!(error.contains("重复属性"), "{error}");

        let multi_document = "model: {}\n---\nmodel: {}\n";
        let error = parse_rendered(multi_document, DocumentFormat::Yaml, "Hermes")
            .err()
            .expect("multi-document YAML fails closed");
        assert!(error.contains("单文档"), "{error}");

        let marker = "vk-sensitive-marker-must-not-leak";
        let invalid = format!("model:\n  api_key: {marker}\n  broken: [\n");
        let error = parse_rendered(&invalid, DocumentFormat::Yaml, "Hermes")
            .err()
            .expect("invalid YAML fails closed");
        assert!(!error.contains(marker), "{error}");
    }

    #[test]
    fn config_owned_projection_preserves_unowned_toml_and_restores_owned_decoration() {
        let current = r#"# current user comment
approval_policy = "on-request"
model = "auto"
model_provider = "tokenstation"

[model_providers.tokenstation]
base_url = "http://127.0.0.1:8787/v1"
wire_api = "responses"

[mcp_servers.user]
command = "keep-me"
"#;
        let baseline = r#"model = "original-model" # original model comment
model_provider = "original-provider"

[model_providers.tokenstation]
# original provider comment
base_url = "https://original.invalid/v1"
custom = "original"

[mcp_servers.user]
command = "must-not-replace-current"
"#;
        let mut current = ConfigDocument::Toml(Box::new(Document::from_str(current).unwrap()));
        let baseline = ConfigDocument::Toml(Box::new(Document::from_str(baseline).unwrap()));
        let paths = [
            ConfigPath {
                segments: vec!["model".to_string()],
            },
            ConfigPath {
                segments: vec!["model_provider".to_string()],
            },
            ConfigPath {
                segments: vec!["model_providers".to_string(), "tokenstation".to_string()],
            },
        ];
        project_owned_paths(&mut current, &baseline, &paths).unwrap();
        let rendered = render_document(&current, "fixture").unwrap();
        assert!(rendered.contains("# current user comment"));
        assert!(rendered.contains("approval_policy = \"on-request\""));
        assert!(rendered.contains("command = \"keep-me\""));
        assert!(!rendered.contains("must-not-replace-current"));
        assert!(rendered.contains("model = \"original-model\" # original model comment"));
        assert!(rendered.contains("# original provider comment"));
        assert!(rendered.contains("custom = \"original\""));
    }
}
