use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rig_agent::agent::{
    AgentHook, DispatchAction, DispatchEvent, HookContext, OutcomeAction, OutcomeEvent,
};
use rig_core::completion::{CompletionResponse, Cost, FinishReason};
use rig_core::effect::{EffectId, EffectKind};
use rig_core::providers::openrouter::extension::OpenRouterExt;

use crate::stats::duration_ms;
use crate::stats::event::{Event, LlmCall, LlmPurpose};
use crate::stats::recorder::RunStats;

/// Records one `llm.call` event for each model call of a rig run.
pub struct StatsHook {
    stats: RunStats,
    purpose: LlmPurpose,
    /// The model that the run requests.
    requested_model: String,
    /// The start of each completion request that has no outcome yet, by its effect.
    started: Mutex<HashMap<EffectId, Instant>>,
}

impl StatsHook {
    pub fn new(stats: RunStats, purpose: LlmPurpose, requested_model: String) -> Self {
        Self {
            stats,
            purpose,
            requested_model,
            started: Mutex::new(HashMap::new()),
        }
    }

    /// The time since the request of `id` started. `on_dispatch` keeps the start of each
    /// completion, so each completion outcome has one.
    fn elapsed(&self, id: EffectId) -> Duration {
        self.started
            .lock()
            .expect("the start times are not poisoned")
            .remove(&id)
            .expect("on_dispatch kept the start of each completion")
            .elapsed()
    }

    /// The `llm.call` event of the model call `turn` (one-based) of the run.
    fn call(&self, turn: usize, elapsed: Duration, response: &CompletionResponse) -> LlmCall {
        let usage = response.usage;
        LlmCall {
            purpose: self.purpose,
            call_index: turn as u64,
            requested_model: self.requested_model.clone(),
            response_model: response.model().map(str::to_string),
            provider: response.extras_lossy::<OpenRouterExt>().provider,
            response_id: response.response_id().map(str::to_string),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            cache_write_tokens: usage.cache_creation_input_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            cost_usd: usage.cost.map(|cost| cost.total),
            cost_estimated: usage.cost.as_ref().is_some_and(is_estimated),
            finish_reason: response.finish_reason().map(finish_reason_name),
            output: serde_json::to_value(&response.choice).expect("the assistant content is JSON"),
            duration_ms: duration_ms(elapsed),
        }
    }
}

impl AgentHook for StatsHook {
    async fn on_dispatch(&self, _ctx: &HookContext, event: DispatchEvent<'_>) -> DispatchAction {
        if is_completion(event.kind) {
            self.started
                .lock()
                .expect("the start times are not poisoned")
                .insert(event.id, Instant::now());
        }
        DispatchAction::proceed()
    }

    async fn on_outcome(&self, _ctx: &HookContext, event: OutcomeEvent<'_>) -> OutcomeAction {
        if !is_completion(event.kind) {
            return OutcomeAction::proceed();
        }
        let elapsed = self.elapsed(event.id);
        // Rig 0.44 calls `on_outcome` for a completion only when the completion succeeded.
        let response = event
            .completion()
            .expect("a completion outcome is a successful completion");
        let call = self.call(event.turn, elapsed, response);
        self.stats.record(None, Event::LlmCall(call)).await;
        OutcomeAction::proceed()
    }
}

fn is_completion(kind: &EffectKind) -> bool {
    matches!(kind, EffectKind::Completion { .. })
}

/// Rig fills the parts of a cost only for an estimate from its price list. This assumes that a
/// cost that the provider reports has only a total, as OpenRouter's has.
fn is_estimated(cost: &Cost) -> bool {
    [cost.input, cost.output, cost.cache_read, cost.cache_write]
        .iter()
        .any(Option::is_some)
}

/// The snake-case name of `reason`, for example `tool_calls`, or the name of the provider for a
/// reason outside the rig vocabulary.
fn finish_reason_name(reason: FinishReason) -> String {
    match reason {
        FinishReason::Stop => "stop".to_string(),
        FinishReason::Length => "length".to_string(),
        FinishReason::ToolCalls => "tool_calls".to_string(),
        FinishReason::ContentFilter => "content_filter".to_string(),
        FinishReason::Other(name) => name,
        unknown => format!("{unknown:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reported_total_is_not_estimated() {
        assert!(!is_estimated(&Cost::from_total(0.0042)));
    }

    #[test]
    fn a_cost_with_parts_is_estimated() {
        assert!(is_estimated(&Cost::from_parts(0.003, 0.001, 0.0002, 0.0)));
    }
}
