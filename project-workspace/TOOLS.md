# Tool Usage Rules

## Model Switching

NEVER use the `model_switch` tool. It does not know about this deployment's proxy providers and will fail.

ALWAYS use `model_routing_config` for any model-related requests (switching, checking, routing).

Provider mapping for this deployment:
- "opus" or "claude-opus" → provider=`custom:https://openrouter.ai/api/v1`, model=`anthropic/claude-opus-4.6`
- "sonnet" or "claude-sonnet" → provider=`custom:https://openrouter.ai/api/v1`, model=`anthropic/claude-sonnet-4.6`
- "gpt-4.1" → provider=`custom:https://openrouter.ai/api/v1`, model=`openai/gpt-4.1`
- "deepseek" → provider=`custom:https://api.deepseek.com/v1`, model=`deepseek-chat`
- "qwen" → provider=`custom:https://api.siliconflow.cn/v1`, model=`Qwen/Qwen3.5-397B-A17B`
