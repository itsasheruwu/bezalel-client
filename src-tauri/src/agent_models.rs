use serde::Serialize;
use serde_json::Value;

pub const FAMILIES: [&str; 4] = ["Astra", "Sol", "Terra", "Luna"];
pub const EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModel {
    pub family: String,
    pub version: Option<String>,
    pub model_id: Option<String>,
    pub efforts: Vec<String>,
}

fn version(slug: &str, family: &str) -> Option<(Vec<u32>, String)> {
    let number = slug
        .strip_prefix("gpt-")?
        .strip_suffix(&format!("-{}", family.to_ascii_lowercase()))?;
    let parts: Vec<u32> = number
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if parts.is_empty() {
        return None;
    }
    Some((parts, number.to_owned()))
}

pub fn catalog(data: &Value) -> Result<Vec<AgentModel>, String> {
    let models = data["models"]
        .as_array()
        .ok_or("The account model catalog is unreadable.")?;
    Ok(FAMILIES
        .iter()
        .map(|family| {
            let latest = models
                .iter()
                .filter(|model| model["visibility"] == "list")
                .filter_map(|model| Some((version(model["slug"].as_str()?, family)?, model)))
                .max_by(|a, b| {
                    let length = a.0 .0.len().max(b.0 .0.len());
                    (0..length)
                        .map(|i| a.0 .0.get(i).copied().unwrap_or(0))
                        .cmp((0..length).map(|i| b.0 .0.get(i).copied().unwrap_or(0)))
                });
            let mut result = AgentModel {
                family: (*family).into(),
                version: None,
                model_id: None,
                efforts: Vec::new(),
            };
            if let Some(((_, number), model)) = latest {
                result.version = Some(number);
                result.model_id = model["slug"].as_str().map(str::to_owned);
                if let Some(efforts) = model
                    .get("supported_reasoning_levels")
                    .or_else(|| model.get("supported_reasoning_efforts"))
                    .and_then(Value::as_array)
                {
                    for allowed in EFFORTS {
                        if efforts.iter().any(|effort| {
                            effort.as_str() == Some(allowed)
                                || effort["reasoning_effort"] == allowed
                                || effort["effort"] == allowed
                        }) {
                            result.efforts.push(allowed.into());
                        }
                    }
                }
                // Older catalogs omit effort metadata; the protocol's common levels.
                if result.efforts.is_empty() {
                    result.efforts = ["low", "medium", "high"].map(str::to_owned).to_vec();
                }
            }
            result
        })
        .collect())
}

pub fn select(
    models: &[AgentModel],
    family: &str,
    effort: &str,
) -> Result<(String, String), String> {
    let model = models
        .iter()
        .find(|model| model.family == family)
        .ok_or("Choose Astra, Sol, Terra, or Luna.")?;
    let id = model
        .model_id
        .clone()
        .ok_or_else(|| format!("{family} is unavailable on this account. Choose another model."))?;
    let effort = if model.efforts.iter().any(|value| value == effort) {
        effort.to_owned()
    } else if model.efforts.iter().any(|value| value == "high") {
        "high".into()
    } else {
        model
            .efforts
            .first()
            .cloned()
            .ok_or("The model has no supported reasoning levels.")?
    };
    Ok((id, effort))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn resolves_latest_per_family_numerically_and_ignores_hidden_and_preview_versions() {
        let data = json!({"models": [
            {"slug":"gpt-6-sol","visibility":"list"},
            {"slug":"gpt-6.9-sol","visibility":"list"},
            {"slug":"gpt-6.10-sol","visibility":"list","supported_reasoning_efforts":[{"reasoning_effort":"low"},{"reasoning_effort":"high"},{"reasoning_effort":"xhigh"}]},
            {"slug":"gpt-8-sol","visibility":"hidden"},
            {"slug":"gpt-9-preview-sol","visibility":"list"},
            {"slug":"gpt-6-astra","visibility":"list"},
            {"slug":"gpt-6.1-terra","visibility":"list"},
            {"slug":"gpt-6.1-luna","visibility":"list"}
        ]});
        let models = catalog(&data).unwrap();
        assert_eq!(models[1].version.as_deref(), Some("6.10"));
        assert_eq!(
            select(&models, "Sol", "xhigh").unwrap(),
            ("gpt-6.10-sol".into(), "xhigh".into())
        );
        assert_eq!(models[2].model_id.as_deref(), Some("gpt-6.1-terra"));
        assert_eq!(select(&models, "Sol", "ultra").unwrap().1, "high");
    }
    #[test]
    fn unavailable_family_is_never_silently_replaced_by_another_model() {
        let models =
            catalog(&json!({"models":[{"slug":"gpt-6.1-sol","visibility":"list"}]})).unwrap();
        assert_eq!(models.len(), 4);
        assert!(select(&models, "Astra", "high")
            .unwrap_err()
            .contains("unavailable"));
    }
}
