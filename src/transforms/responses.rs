//! Stateless compatibility with the subscription Responses transport.
// JSON reads return Null for missing fields; writes below target validated or constructed objects.
#![allow(clippy::indexing_slicing)]
use llm_relay::Usage;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

type Validation<T> = Result<T, String>;

pub fn prepare(mut body: Value) -> Validation<Value> {
    let obj = body.as_object_mut().ok_or("Expected a JSON object")?;
    if obj.get("store").and_then(Value::as_bool) == Some(true) {
        return Err("Subscription requests require store=false".into());
    }
    for field in [
        "previous_response_id",
        "conversation",
        "background",
        "max_output_tokens",
        "temperature",
        "top_p",
        "truncation",
    ] {
        if obj
            .get(field)
            .is_some_and(|v| !v.is_null() && v != &Value::Bool(false))
        {
            return Err(format!(
                "{field} is not supported by this subscription endpoint; send full conversation history"
            ));
        }
        obj.remove(field);
    }
    match obj.get_mut("input") {
        Some(input) if input.is_string() => {
            *input = json!([{"role":"user","content":input.as_str()}])
        }
        Some(input) if input.is_array() => {}
        _ => return Err("input must be a string or array".into()),
    }
    if obj.get("instructions").is_none_or(Value::is_null) {
        obj.insert("instructions".into(), json!(""));
    }
    if !obj.get("instructions").is_some_and(Value::is_string) {
        return Err("instructions must be a string".into());
    }
    if let Some(v) = obj.get("stream")
        && !v.is_boolean()
    {
        return Err("stream must be a boolean".into());
    }
    obj.insert("store".into(), json!(false));
    obj.insert("stream".into(), json!(true));
    Ok(body)
}

pub fn from_chat(body: &Value) -> Validation<Value> {
    let obj = body.as_object().ok_or("Expected a JSON object")?;
    let allowed = [
        "model",
        "messages",
        "stream",
        "stream_options",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "reasoning_effort",
        "response_format",
        "n",
        "store",
        "user",
        "prompt_cache_key",
    ];
    for (key, value) in obj {
        if !allowed.contains(&key.as_str()) && !value.is_null() {
            return Err(format!("Unsupported ChatGPT chat parameter: {key}"));
        }
    }
    if obj
        .get("n")
        .is_some_and(|n| !n.is_null() && n.as_u64() != Some(1))
    {
        return Err("Only n=1 is supported".into());
    }
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("messages must be an array")?;
    if messages.is_empty() {
        return Err("messages must not be empty".into());
    }
    let mut input = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or("Missing message role")?;
        if role == "tool" {
            let id = message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .ok_or("Tool result requires tool_call_id")?;
            let output = message
                .get("content")
                .and_then(Value::as_str)
                .ok_or("Tool result content must be text")?;
            input.push(json!({"type":"function_call_output","call_id":id,"output":output}));
            continue;
        }
        if !["user", "assistant", "system", "developer"].contains(&role) {
            return Err(format!("Unsupported role: {role}"));
        }
        let mut content = Vec::new();
        match message.get("content") {
            Some(Value::String(text)) => content.push(
                json!({"type":if role=="assistant" {"output_text"}else{"input_text"},"text":text}),
            ),
            Some(Value::Array(parts)) => {
                for part in parts {
                    match part.get("type").and_then(Value::as_str) {
                    Some("text") => content.push(json!({"type":if role=="assistant" {"output_text"}else{"input_text"},"text":part.get("text").and_then(Value::as_str).ok_or("Text part requires text")?})),
                    Some("image_url") if role == "user" => {
                        let url = part.pointer("/image_url/url").and_then(Value::as_str).ok_or("Image requires a URL")?;
                        content.push(json!({"type":"input_image","image_url":url,"detail":part.pointer("/image_url/detail").and_then(Value::as_str).unwrap_or("auto")}));
                    },
                    _ => return Err("Unsupported chat content part".into()),
                }
                }
            }
            None | Some(Value::Null) if role == "assistant" => {}
            _ => return Err("Invalid message content".into()),
        }
        if !content.is_empty() {
            input.push(json!({"role":if role=="system" {"developer"}else{role},"content":content}));
        }
        if let Some(calls) = message.get("tool_calls").filter(|v| !v.is_null()) {
            if role != "assistant" {
                return Err("Only assistant messages may contain tool_calls".into());
            }
            for call in calls.as_array().ok_or("tool_calls must be an array")? {
                if call.get("type").and_then(Value::as_str) != Some("function") {
                    return Err("Only function calls are supported".into());
                }
                input.push(json!({"type":"function_call","call_id":call.get("id").and_then(Value::as_str).ok_or("Missing tool call id")?,"name":call.pointer("/function/name").and_then(Value::as_str).ok_or("Missing function name")?,"arguments":call.pointer("/function/arguments").and_then(Value::as_str).ok_or("Missing function arguments")?}));
            }
        }
    }
    let mut result = json!({"model":body.get("model"),"input":input});
    for field in ["stream", "store", "parallel_tool_calls", "prompt_cache_key"] {
        if let Some(v) = body.get(field) {
            result[field] = v.clone();
        }
    }
    if let Some(effort) = body.get("reasoning_effort").filter(|v| !v.is_null()) {
        result["reasoning"] = json!({"effort":effort});
    }
    if let Some(tools) = body.get("tools").filter(|v| !v.is_null()) {
        let mut output = Vec::new();
        for tool in tools.as_array().ok_or("tools must be an array")? {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Err(
                    "Only function tools are supported in chat; use Responses for native tools"
                        .into(),
                );
            }
            let mut function = tool
                .get("function")
                .filter(|v| v.is_object())
                .ok_or("Missing function definition")?
                .clone();
            function["type"] = json!("function");
            output.push(function);
        }
        result["tools"] = json!(output);
    }
    if let Some(choice) = body.get("tool_choice") {
        result["tool_choice"] = if choice.is_object() {
            json!({"type":"function","name":choice.pointer("/function/name").and_then(Value::as_str).ok_or("Missing tool_choice name")?})
        } else {
            choice.clone()
        };
    }
    if let Some(format) = body.get("response_format").filter(|v| !v.is_null()) {
        result["text"] = json!({"format":match format.get("type").and_then(Value::as_str) {
            Some("json_schema") => { let mut schema=format.get("json_schema").filter(|v|v.is_object()).ok_or("Missing json_schema")?.clone(); schema["type"]=json!("json_schema"); schema },
            Some("text" | "json_object") => format.clone(),
            _ => return Err("Unsupported response_format".into()),
        }});
    }
    prepare(result)
}

pub fn usage(response: &Value) -> Option<Usage> {
    let u = response.get("usage").filter(|v| v.is_object())?;
    let input = u.get("input_tokens").and_then(Value::as_u64)?;
    let cached = u
        .pointer("/input_tokens_details/cached_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(input);
    Some(Usage {
        input_tokens: input.saturating_sub(cached),
        output_tokens: u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
        cache_read_input_tokens: Some(cached),
        cache_creation_input_tokens: None,
    })
}
fn chat_usage(response: &Value) -> Value {
    let u = &response["usage"];
    json!({"prompt_tokens":u["input_tokens"],"completion_tokens":u["output_tokens"],"total_tokens":u["total_tokens"],"prompt_tokens_details":u["input_tokens_details"],"completion_tokens_details":u["output_tokens_details"]})
}
fn finish_reason(response: &Value, tools: bool) -> &'static str {
    if response["status"] == "incomplete" {
        "length"
    } else if tools {
        "tool_calls"
    } else {
        "stop"
    }
}
pub fn to_chat(response: &Value) -> Value {
    let mut text = String::new();
    let mut refusal = String::new();
    let mut calls = Vec::new();
    if let Some(output) = response["output"].as_array() {
        for item in output {
            if item["type"] == "function_call" {
                calls.push(json!({"id":item["call_id"],"type":"function","function":{"name":item["name"],"arguments":item["arguments"]}}));
            }
            if let Some(parts) = item["content"].as_array() {
                for part in parts {
                    if part["type"] == "output_text" {
                        text.push_str(part["text"].as_str().unwrap_or(""));
                    }
                    if part["type"] == "refusal" {
                        refusal.push_str(part["refusal"].as_str().unwrap_or(""));
                    }
                }
            }
        }
    }
    let mut message =
        json!({"role":"assistant","content":if text.is_empty(){Value::Null}else{json!(text)}});
    if !calls.is_empty() {
        message["tool_calls"] = json!(calls);
    }
    if !refusal.is_empty() {
        message["refusal"] = json!(refusal);
    }
    json!({"id":response["id"],"object":"chat.completion","created":response["created_at"],"model":response["model"],"choices":[{"index":0,"message":message,"finish_reason":finish_reason(response,!calls.is_empty())}],"usage":chat_usage(response)})
}

/// Terminal responses sometimes omit output; retain completed items by output index.
#[derive(Default)]
pub struct Accumulator {
    items: BTreeMap<u64, Value>,
}
impl Accumulator {
    pub fn observe(&mut self, event: &mut Value) {
        if event["type"] == "response.output_item.done"
            && let Some(index) = event["output_index"].as_u64()
        {
            self.items.insert(index, event["item"].clone());
        }
        if matches!(
            event["type"].as_str(),
            Some("response.completed" | "response.incomplete" | "response.failed")
        ) && event
            .pointer("/response/output")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
            && let Some(response) = event.get_mut("response").and_then(Value::as_object_mut)
        {
            response.insert(
                "output".into(),
                json!(self.items.values().collect::<Vec<_>>()),
            );
        }
    }
}

pub struct ChatStream {
    id: Value,
    created: Value,
    model: String,
    calls: HashMap<u64, usize>,
    include_usage: bool,
}
impl ChatStream {
    pub fn new(model: String, include_usage: bool) -> Self {
        Self {
            id: json!(format!("chatcmpl-{}", uuid::Uuid::new_v4())),
            created: json!(chrono::Utc::now().timestamp()),
            model,
            calls: HashMap::new(),
            include_usage,
        }
    }
    fn chunk(&self, delta: Value, finish: Value) -> Value {
        json!({"id":self.id,"object":"chat.completion.chunk","created":self.created,"model":self.model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    }
    pub fn event(&mut self, event: &Value) -> Vec<Value> {
        let mut chunks = Vec::new();
        match event["type"].as_str().unwrap_or("") {
            "response.created" => {
                self.id = event["response"]["id"].clone();
                self.created = event["response"]["created_at"].clone();
                chunks.push(self.chunk(json!({"role":"assistant","content":""}), Value::Null));
            }
            "response.output_text.delta" => {
                chunks.push(self.chunk(json!({"content":event["delta"]}), Value::Null))
            }
            "response.refusal.delta" => {
                chunks.push(self.chunk(json!({"refusal":event["delta"]}), Value::Null))
            }
            "response.reasoning_summary_text.delta" => {
                chunks.push(self.chunk(json!({"reasoning_content":event["delta"]}), Value::Null))
            }
            "response.output_item.added" if event["item"]["type"] == "function_call" => {
                if let Some(index) = event["output_index"].as_u64() {
                    let next = self.calls.len();
                    let slot = *self.calls.entry(index).or_insert(next);
                    chunks.push(self.chunk(json!({"tool_calls":[{"index":slot,"id":event["item"]["call_id"],"type":"function","function":{"name":event["item"]["name"],"arguments":event["item"]["arguments"].as_str().unwrap_or("")}}]}),Value::Null));
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(slot) = event["output_index"]
                    .as_u64()
                    .and_then(|i| self.calls.get(&i))
                {
                    chunks.push(self.chunk(json!({"tool_calls":[{"index":slot,"function":{"arguments":event["delta"]}}]}),Value::Null));
                }
            }
            "response.completed" | "response.incomplete" => {
                chunks.push(self.chunk(
                    json!({}),
                    json!(finish_reason(&event["response"], !self.calls.is_empty())),
                ));
                if self.include_usage {
                    let mut chunk = self.chunk(json!({}), Value::Null);
                    chunk["choices"] = json!([]);
                    chunk["usage"] = chat_usage(&event["response"]);
                    chunks.push(chunk);
                }
            }
            _ => {}
        }
        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chat_preserves_tool_roundtrip_and_images() {
        let b=from_chat(&json!({"model":"gpt-test","messages":[
            {"role":"system","content":"Be concise"},
            {"role":"user","content":[{"type":"text","text":"Look"},{"type":"image_url","image_url":{"url":"data:image/png;base64,abc"}}]},
            {"role":"assistant","content":null,"tool_calls":[{"id":"call_a","type":"function","function":{"name":"lookup","arguments":"{\"x\":1}"}}]},
            {"role":"tool","tool_call_id":"call_a","content":"42"}],
            "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object","properties":{}}}}],
            "tool_choice":{"type":"function","function":{"name":"lookup"}},"reasoning_effort":"high"
        })).unwrap();
        assert_eq!(b.pointer("/input/0/role"), Some(&json!("developer")));
        assert_eq!(
            b.pointer("/input/1/content/1/type"),
            Some(&json!("input_image"))
        );
        assert_eq!(b.pointer("/input/2/call_id"), Some(&json!("call_a")));
        assert_eq!(b.pointer("/input/3/output"), Some(&json!("42")));
        assert_eq!(b.pointer("/tools/0/name"), Some(&json!("lookup")));
        assert_eq!(b.pointer("/tool_choice/name"), Some(&json!("lookup")));
        assert_eq!(b["stream"], true);
        assert_eq!(b["store"], false);
    }
    #[test]
    fn rejects_unsupported_controls_instead_of_silently_changing_them() {
        for extra in [
            json!({"n":2}),
            json!({"max_tokens":100}),
            json!({"temperature":0}),
            json!({"store":true}),
        ] {
            let mut b = json!({"model":"gpt-test","messages":[{"role":"user","content":"hi"}]});
            b.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(from_chat(&b).is_err());
        }
        assert!(prepare(json!({"input":"hi","previous_response_id":"resp_old"})).is_err());
    }
    #[test]
    fn completes_missing_output_in_order_and_preserves_native_items() {
        let mut a = Accumulator::default();
        a.observe(&mut json!({"type":"response.output_item.done","output_index":1,"item":{"type":"message","content":[{"type":"output_text","text":"Привет"}]}}));
        a.observe(&mut json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","encrypted_content":"opaque"}}));
        let mut e = json!({"type":"response.completed","response":{"output":[]}});
        a.observe(&mut e);
        assert_eq!(
            e.pointer("/response/output/0/encrypted_content"),
            Some(&json!("opaque"))
        );
        assert_eq!(
            to_chat(&e["response"]).pointer("/choices/0/message/content"),
            Some(&json!("Привет"))
        );
    }
    #[test]
    fn cached_tokens_are_not_double_counted() {
        let usage=usage(&json!({"usage":{"input_tokens":100,"input_tokens_details":{"cached_tokens":80},"output_tokens":12}})).unwrap();
        assert_eq!(usage.input_tokens, 20);
        assert_eq!(usage.cache_read_input_tokens, Some(80));
        assert_eq!(usage.output_tokens, 12);
    }
    #[test]
    fn tool_stream_indices_are_dense_and_usage_has_empty_choices() {
        let mut s = ChatStream::new("gpt-test".into(), true);
        for (output_index, expected) in [(2, 0), (4, 1)] {
            let chunks=s.event(&json!({"type":"response.output_item.added","output_index":output_index,"item":{"type":"function_call","call_id":format!("call_{expected}"),"name":"lookup","arguments":""}}));
            assert_eq!(
                chunks
                    .first()
                    .unwrap()
                    .pointer("/choices/0/delta/tool_calls/0/index"),
                Some(&json!(expected))
            );
        }
        let chunks = s.event(
            &json!({"type":"response.function_call_arguments.delta","output_index":4,"delta":"{}"}),
        );
        assert_eq!(
            chunks
                .first()
                .unwrap()
                .pointer("/choices/0/delta/tool_calls/0/index"),
            Some(&json!(1))
        );
        let chunks=s.event(&json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":10,"output_tokens":20,"total_tokens":30}}}));
        assert_eq!(
            chunks.first().unwrap().pointer("/choices/0/finish_reason"),
            Some(&json!("tool_calls"))
        );
        assert_eq!(chunks.last().unwrap()["choices"], json!([]));
        assert_eq!(chunks.last().unwrap()["usage"]["total_tokens"], 30);
    }
}
