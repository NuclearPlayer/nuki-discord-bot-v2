use crate::store::settings::ModelSelection;

pub(crate) fn selection(model_id: &str, input_modalities: &[&str]) -> ModelSelection {
    ModelSelection {
        model_id: model_id.to_string(),
        input_modalities: input_modalities.iter().map(|m| m.to_string()).collect(),
    }
}
