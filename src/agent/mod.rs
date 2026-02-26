//! ReAct-style agent loop: Thought → Action → Observation → ...
//!
//! Activated when the `AGENT_MODE=true` environment variable is set.
//! The agent uses the LLM to decide which plugin/tool to call, invokes it,
//! feeds the observation back, and repeats until the LLM emits a final answer
//! or the step limit is reached.
//!
//! Prompt format (XML-delimited for easy parsing):
//! ```
//! <thought>I should check the weather in Milan.</thought>
//! <action>plugin-weather</action>
//! <action_input>{"args": "Milan"}</action_input>
//! ```
//! After injecting the observation the loop continues:
//! ```
//! <observation>{"location":"Milan, IT","temperature":"18.0°C",...}</observation>
//! <thought>I now have the answer.</thought>
//! <answer>The weather in Milan is 18°C and sunny.</answer>
//! ```

use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tracing::{info, warn, debug};

use crate::errors::AppError;
use crate::llm::LlmActor;
use crate::plugins::{PluginRegistry, PluginRequest, PluginRunner};
use crate::security::DeviceIdentity;

const MAX_STEPS: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStep {
    pub thought: String,
    pub action: Option<String>,
    pub action_input: Option<serde_json::Value>,
    pub observation: Option<String>,
    pub answer: Option<String>,
}

pub struct AgentRunner {
    llm: Arc<LlmActor>,
    plugins: Arc<PluginRegistry>,
    device: Arc<DeviceIdentity>,
    plugin_runner: PluginRunner,
}

impl AgentRunner {
    pub fn new(
        llm: Arc<LlmActor>,
        plugins: Arc<PluginRegistry>,
        device: Arc<DeviceIdentity>,
        plugin_dir: &str,
    ) -> Self {
        let plugin_runner = PluginRunner::new(plugin_dir);
        Self { llm, plugins, device, plugin_runner }
    }

    /// Run the ReAct loop for a user query.
    /// Returns the final answer string and all intermediate steps.
    pub async fn run(
        &self,
        user_query: &str,
        system_prompt: Option<&str>,
        max_tokens: u32,
        temperature: f32,
    ) -> Result<(String, Vec<AgentStep>), AppError> {
        let tools_desc = self.build_tools_description();
        let sys = system_prompt.unwrap_or("You are a helpful assistant.");

        // Seed prompt
        let mut prompt = format!(
            "<|system|>\n{sys}\n\
             You have access to the following tools:\n{tools_desc}\n\n\
             To use a tool respond ONLY with:\n\
             <thought>your reasoning</thought>\n\
             <action>tool-name</action>\n\
             <action_input>{{\"args\": \"...\"}}</action_input>\n\n\
             When you have a final answer respond ONLY with:\n\
             <thought>reasoning</thought>\n\
             <answer>your final answer</answer>\n\
             <|user|>\n{user_query}\n\
             <|assistant|>\n"
        );

        let mut steps: Vec<AgentStep> = Vec::new();

        for step_num in 0..MAX_STEPS {
            debug!(step = step_num, "Agent step");

            let raw = self.llm
                .infer(prompt.clone(), max_tokens, temperature)
                .await?;

            debug!(raw = %raw, "LLM raw output");

            let parsed = parse_react_output(&raw);

            if let Some(answer) = &parsed.answer {
                info!(steps = step_num, "Agent finished with answer");
                steps.push(parsed.clone());
                return Ok((answer.clone(), steps));
            }

            if let (Some(action), Some(action_input)) = (&parsed.action, &parsed.action_input) {
                // Resolve tool
                let observation = self.invoke_tool(action, action_input).await;
                info!(action = %action, ok = observation.starts_with('{'), "Tool observation");

                let obs_json = observation.clone();
                let step_with_obs = AgentStep {
                    thought: parsed.thought.clone(),
                    action: Some(action.clone()),
                    action_input: Some(action_input.clone()),
                    observation: Some(obs_json.clone()),
                    answer: None,
                };
                steps.push(step_with_obs);

                // Append LLM output + observation to the running prompt
                prompt.push_str(&raw);
                prompt.push_str(&format!("\n<observation>{}</observation>\n", obs_json));
                continue;
            }

            // No action and no answer — LLM is confused; emit what we have
            warn!(step = step_num, "LLM produced neither action nor answer — stopping");
            steps.push(parsed.clone());
            return Ok((parsed.thought.clone(), steps));
        }

        warn!(max_steps = MAX_STEPS, "Agent hit step limit");
        Ok(("I've reached my step limit. Please rephrase or break your request into smaller parts.".into(), steps))
    }

    async fn invoke_tool(
        &self,
        action: &str,
        action_input: &serde_json::Value,
    ) -> String {
        // Try to resolve as slash-command or plugin name
        let manifest_opt = self.plugins
            .resolve(action)
            .or_else(|| self.plugins.resolve_by_name(action));

        let manifest = match manifest_opt {
            Some(m) => m.clone(),
            None => return format!("{{\"error\": \"Unknown tool '{}'.\"}}", action),
        };

        // Standardise payload: always {"args": "..."}
        let args = action_input.get("args")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| action_input.to_string());

        let request = PluginRequest {
            action: manifest.default_action.clone(),
            payload: serde_json::json!({ "args": args }),
        };

        match self.plugin_runner.run(&manifest, &request, &self.device).await {
            Ok(resp) if resp.success => resp.result.to_string(),
            Ok(resp) => format!(
                "{{\"error\": \"{}\"}}",
                resp.error.unwrap_or_else(|| "plugin error".into())
            ),
            Err(e) => format!("{{\"error\": \"{}\"}}", e),
        }
    }

    fn build_tools_description(&self) -> String {
        let tools = self.plugins.as_tools();
        if tools.is_empty() { return "(no tools available)".into(); }
        tools.iter()
            .filter_map(|t| {
                let name = t["function"]["name"].as_str()?;
                let desc = t["function"]["description"].as_str()?;
                Some(format!("- {name}: {desc}"))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ── ReAct output parser ───────────────────────────────────────────────────────

fn parse_react_output(text: &str) -> AgentStep {
    AgentStep {
        thought: extract_tag(text, "thought").unwrap_or_else(|| text.trim().to_string()),
        action: extract_tag(text, "action"),
        action_input: extract_tag(text, "action_input")
            .and_then(|s| serde_json::from_str(&s).ok()),
        observation: extract_tag(text, "observation"),
        answer: extract_tag(text, "answer"),
    }
}

fn extract_tag(text: &str, tag: &str) -> Option<String> {
    let open  = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = text.find(&open)? + open.len();
    let end   = text[start..].find(&close)? + start;
    Some(text[start..end].trim().to_string())
}
