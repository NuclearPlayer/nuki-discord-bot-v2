use serde_json::{Map, Value, json};

pub(crate) fn category_probabilities(probabilities: &[(&str, f64)]) -> Map<String, Value> {
    probabilities
        .iter()
        .map(|(name, probability)| (name.to_string(), json!(probability)))
        .collect()
}

pub(crate) fn category_answer(probabilities: &[(&str, f64)]) -> Value {
    let (choice, confidence) = probabilities
        .iter()
        .copied()
        .max_by(|(_, left), (_, right)| left.total_cmp(right))
        .unwrap();
    json!({
        "category": {
            "type": "choice",
            "choice": choice,
            "confidence": confidence,
            "probabilities": category_probabilities(probabilities),
        }
    })
}
