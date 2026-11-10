use rig_agent::agent::{AgentHook, DispatchAction, DispatchEvent, HookContext};
use rig_core::completion::CompletionRequest;
use rig_core::effect::EffectKind;
use rig_core::message::{Message, ToolResultContent, UserContent};

use crate::prompts::files::EXAMPLES;

/// Adds the reply examples of `prompts/examples.md` to the last message of each model request
/// of a rig run. The examples are then the last text that the model reads before it writes.
///
/// The hook changes only the request that rig sends, not the messages of the run. Rig makes
/// each request from the messages of the run, so only the last message of a request has the
/// examples. `TurnResult::turn_messages` does not contain them.
pub struct ExamplesHook;

impl AgentHook for ExamplesHook {
    async fn on_dispatch(&self, _ctx: &HookContext, event: DispatchEvent<'_>) -> DispatchAction {
        let EffectKind::Completion { request, stream } = event.kind else {
            return DispatchAction::proceed();
        };
        DispatchAction::patch(EffectKind::Completion {
            request: with_examples(request.clone()),
            stream: *stream,
        })
    }
}

/// Adds the examples as the last text of the last message of `request`. If that message ends
/// with a tool result, the examples go into the tool result, because the provider gets each
/// tool result as one message. If the last message is not a user message, changes nothing.
fn with_examples(mut request: CompletionRequest) -> CompletionRequest {
    if let Some(Message::User { content }) = request.chat_history.last_mut() {
        match content.last_mut() {
            Some(UserContent::ToolResult(result)) => {
                result
                    .content
                    .push(ToolResultContent::text(examples_reminder()));
            }
            _ => content.push(UserContent::text(examples_reminder())),
        }
    }
    request
}

/// Returns the examples in `<system-reminder>` tags.
fn examples_reminder() -> String {
    format!("<system-reminder>\n{}\n</system-reminder>", EXAMPLES.trim())
}
