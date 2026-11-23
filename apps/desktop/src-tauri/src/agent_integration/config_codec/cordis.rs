//! Cordis overlays select stable row IDs, not mutable sequence positions.
use super::*;

pub(super) fn validate(rendered: &str) -> Result<Value, String> {
    let root = strict_yaml_semantic(rendered, "Cordis overlay")?;
    let rows = root
        .as_array()
        .ok_or("Cordis overlay must contain a YAML array.")?;
    let mut ids = std::collections::BTreeSet::new();
    for row in rows {
        let id = row
            .as_object()
            .and_then(|row| row.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or("Each Cordis row requires a nonempty string ID.")?;
        if !ids.insert(id) {
            return Err("Cordis overlay contains duplicate row IDs.".into());
        }
    }
    YamlEditDocument::from_str(rendered)
        .map_err(|_| "Cordis overlay syntax is invalid.")?
        .as_sequence()
        .ok_or("Cordis overlay must contain a YAML sequence.")?;
    Ok(root)
}

pub(super) fn at<'a>(root: &'a Value, segments: &[String]) -> Option<&'a Value> {
    let [prefix, id, rest @ ..] = segments else {
        return None;
    };
    if prefix != "$cordis" {
        return None;
    }
    let row = root
        .as_array()?
        .iter()
        .find(|row| row.get("id").and_then(Value::as_str) == Some(id))?;
    rest.iter()
        .try_fold(row, |value, key| value.as_object()?.get(key))
}

pub(super) fn apply(
    document: &mut LosslessYamlDocument,
    operation: &PatchOperation,
) -> Result<(), String> {
    split_path(&operation.path)?;
    let [prefix, id, rest @ ..] = operation.path.segments.as_slice() else {
        return Err("Invalid Cordis ownership path.".into());
    };
    if prefix != "$cordis" || (!rest.is_empty() && rest[0] != "config") {
        return Err("Invalid Cordis ownership path.".into());
    }
    let semantic = validate(&document.rendered)?;
    if operation.operation == PatchKind::Test {
        return (at(&semantic, &operation.path.segments) == operation.value.as_ref())
            .then_some(())
            .ok_or_else(|| "Cordis owned value changed.".into());
    }
    let rows = semantic.as_array().unwrap();
    let index = rows.iter().position(|row| row["id"].as_str() == Some(id));
    if index.is_none() && operation.operation == PatchKind::Remove {
        return Ok(());
    }
    let mut row = index
        .map(|i| rows[i].clone())
        .unwrap_or_else(|| json!({"id":id}));
    if rest.is_empty() {
        if operation.operation != PatchKind::Remove {
            row = operation.value.clone().ok_or("Missing Cordis row value.")?;
            if row.get("id").and_then(Value::as_str) != Some(id) {
                return Err("Cordis row ID must match its ownership path.".into());
            }
        }
    } else {
        let op = PatchOperation {
            path: ConfigPath {
                segments: rest.to_vec(),
            },
            operation: operation.operation,
            value: operation.value.clone(),
        };
        if matches!(operation.operation, PatchKind::Add | PatchKind::Replace) {
            promote_json_null_ancestors(&mut row, &rest[..rest.len() - 1])?;
        }
        apply_json_operation(&mut row, &op)?;
    }
    let tree = YamlEditDocument::from_str(&document.rendered)
        .map_err(|_| "Cannot edit Cordis overlay.")?;
    let sequence = tree.as_sequence().ok_or("Invalid Cordis sequence.")?;
    let remove = rest.is_empty() && operation.operation == PatchKind::Remove;
    let mut rendered = document.rendered.clone();
    if sequence.is_flow_style() {
        // JSON arrays have no comments. Preserve commented YAML by refusing flow edits.
        let mut all: Vec<Value> = serde_json::from_str(&rendered)
            .map_err(|_| "Use a block sequence or JSON array for the Cordis overlay.")?;
        match (index, remove) {
            (Some(index), true) => {
                all.remove(index);
            }
            (Some(index), false) => all[index] = row.clone(),
            (None, false) => all.push(row.clone()),
            (None, true) => {}
        }
        rendered =
            serde_json::to_string_pretty(&all).map_err(|_| "Cannot encode Cordis overlay.")?;
        rendered.push('\n');
    } else if let Some(index) = index {
        let item = sequence.get(index).ok_or("Missing Cordis row.")?;
        let mapping = item.as_mapping().ok_or("Invalid Cordis row.")?;
        let range = mapping.byte_range();
        let start = range.start as usize;
        let end = range.end as usize;
        let line_start = rendered[..start].rfind('\n').map_or(0, |i| i + 1);
        if rendered[line_start..start].trim() != "-" {
            return Err("Cordis rows require a block sequence marker.".into());
        }
        if remove {
            rendered.replace_range(line_start..end, "");
        } else {
            let raw = &rendered[start..end];
            let indentation = start - line_start;
            let mut original = String::new();
            for (n, line) in raw.split_inclusive('\n').enumerate() {
                original.push_str(if n == 0 {
                    line
                } else {
                    line.strip_prefix(&" ".repeat(indentation)).unwrap_or(line)
                });
            }
            let mut edited = LosslessYamlDocument { rendered: original };
            if rest.is_empty() || serde_json::from_str::<Value>(&edited.rendered).is_ok() {
                edited.rendered =
                    serde_json::to_string(&row).map_err(|_| "Cannot encode Cordis row.")?;
                if raw.ends_with('\n') {
                    edited.rendered.push('\n');
                }
            } else {
                // Expand null ancestors without widening ownership. Reverse operations restore null.
                if matches!(operation.operation, PatchKind::Add | PatchKind::Replace) {
                    for length in 1..rest.len() {
                        let path = ConfigPath {
                            segments: rest[..length].to_vec(),
                        };
                        if semantic_value_at(
                            &strict_yaml_semantic(&edited.rendered, "Cordis row")?,
                            &path,
                        )
                        .is_some_and(Value::is_null)
                        {
                            apply_yaml_operation(
                                &mut edited,
                                &PatchOperation {
                                    operation: PatchKind::Replace,
                                    path,
                                    value: Some(json!({})),
                                },
                            )?;
                        }
                    }
                }
                let op = PatchOperation {
                    path: ConfigPath {
                        segments: rest.to_vec(),
                    },
                    operation: operation.operation,
                    value: operation.value.clone(),
                };
                if let Err(error) = apply_yaml_operation(&mut edited, &op) {
                    // A nested flow mapping is one YAML scalar to the lossless block editor.
                    // Replace only that scalar with its merged value, keeping the surrounding trivia.
                    let mut done = false;
                    for length in 1..rest.len() {
                        let tree = YamlEditDocument::from_str(&edited.rendered)
                            .map_err(|_| "Invalid Cordis row.")?;
                        let mut child = tree.get(rest[0].as_str());
                        for key in &rest[1..length] {
                            child = child.and_then(|value| {
                                value.as_mapping().and_then(|map| map.get(key.as_str()))
                            });
                        }
                        if child
                            .as_ref()
                            .and_then(|value| value.as_mapping())
                            .is_some_and(|map| map.is_flow_style())
                        {
                            if child
                                .as_ref()
                                .and_then(|value| value.as_mapping())
                                .is_some_and(|map| map.to_string().contains('#'))
                            {
                                return Err("Use block mappings to preserve comments inside Cordis configuration.".into());
                            }
                            let path = ConfigPath {
                                segments: rest[..length].to_vec(),
                            };
                            let value = semantic_value_at(&row, &path)
                                .cloned()
                                .ok_or("Missing merged Cordis configuration.")?;
                            apply_yaml_operation(
                                &mut edited,
                                &PatchOperation {
                                    operation: PatchKind::Replace,
                                    path,
                                    value: Some(value),
                                },
                            )?;
                            done = true;
                            break;
                        }
                    }
                    if !done {
                        return Err(error);
                    }
                }
            }
            let mut replacement = String::new();
            for (n, line) in edited.rendered.split_inclusive('\n').enumerate() {
                if n > 0 {
                    replacement.push_str(&" ".repeat(indentation));
                }
                replacement.push_str(line);
            }
            rendered.replace_range(start..end, &replacement);
        }
    } else {
        if !rendered.ends_with('\n') {
            rendered.push('\n');
        }
        rendered.push_str("- ");
        rendered.push_str(&serde_json::to_string(&row).map_err(|_| "Cannot encode Cordis row.")?);
        rendered.push('\n');
    }
    if remove && rows.len() == 1 && !sequence.is_flow_style() {
        // Keep comments outside the removed row when restoring an empty overlay.
        rendered.push_str("[]\n");
    }
    let mut expected = rows.clone();
    match (index, remove) {
        (Some(index), true) => {
            expected.remove(index);
        }
        (Some(index), false) => expected[index] = row,
        (None, false) => expected.push(row),
        (None, true) => {}
    }
    if validate(&rendered)? != Value::Array(expected) {
        return Err("Cordis edit changed an unowned value.".into());
    }
    document.rendered = rendered;
    Ok(())
}

pub(super) fn project(
    current: &mut LosslessYamlDocument,
    baseline: &LosslessYamlDocument,
    paths: &[ConfigPath],
) -> Result<(), String> {
    let source = validate(&baseline.rendered)?;
    for path in paths {
        let value = at(&source, &path.segments).cloned();
        apply(
            current,
            &PatchOperation {
                operation: if value.is_some() {
                    PatchKind::Replace
                } else {
                    PatchKind::Remove
                },
                path: path.clone(),
                value,
            },
        )?;
    }
    let mut ancestors = paths
        .iter()
        .flat_map(|path| (2..path.segments.len()).map(|len| path.segments[..len].to_vec()))
        .collect::<Vec<_>>();
    ancestors.sort_by_key(|path| std::cmp::Reverse(path.len()));
    ancestors.dedup();
    for segments in ancestors {
        let before = at(&source, &segments);
        if before.is_some_and(|v| !v.is_null()) {
            continue;
        }
        let semantic = validate(&current.rendered)?;
        let empty = at(&semantic, &segments).is_some_and(|v| {
            v.as_object().is_some_and(|object| {
                object.is_empty()
                    || (segments.len() == 2 && object.len() == 1 && object.contains_key("id"))
            })
        });
        if empty {
            apply(
                current,
                &PatchOperation {
                    operation: if before.is_some() {
                        PatchKind::Replace
                    } else {
                        PatchKind::Remove
                    },
                    path: ConfigPath { segments },
                    value: before.cloned(),
                },
            )?;
        }
    }
    Ok(())
}
