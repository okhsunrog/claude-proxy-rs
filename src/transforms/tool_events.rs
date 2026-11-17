//! Restore adapted arguments after complete JSON arrives, shared by both SSE APIs.
use super::{
    tool_adapters::ArgumentAdapter,
    tool_aliases::{ToolNameMap, restore_response_tool_names},
};
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use std::{collections::HashMap, io};

const MAX_ARGUMENT_BYTES: usize = 4 * 1024 * 1024;

pub fn restore(
    events: impl Stream<Item = Result<Value, io::Error>> + Send,
    map: ToolNameMap,
) -> impl Stream<Item = Result<Value, io::Error>> + Send {
    async_stream::stream! {
        let events=std::pin::pin!(events);
        let mut events=events;
        let mut pending: HashMap<u64,(ArgumentAdapter,String)>=HashMap::new();
        while let Some(event)=events.next().await {
            let mut event=match event {Ok(event)=>event,Err(e)=>{yield Err(e);return;}};
            let kind=event.get("type").and_then(Value::as_str).unwrap_or("").to_owned();
            let index=event.get("index").and_then(Value::as_u64).unwrap_or(0);
            if kind=="content_block_start" && let Some(block)=event.get_mut("content_block") {
                if block.get("type").and_then(Value::as_str)==Some("tool_use") {
                    let adapter=block.get("name").and_then(Value::as_str).and_then(|n|map.adapter(n)).filter(|a|a.changes_arguments()).cloned();
                    let empty=block.get("input").is_none_or(|i|i.as_object().is_some_and(|o|o.is_empty()));
                    if let Some(adapter)=adapter && empty {pending.insert(index,(adapter,String::new()));}
                    let mut message=json!({"content":[block.clone()]});
                    restore_response_tool_names(&mut message,&map);
                    if let Some(restored)=message.pointer("/content/0") {*block=restored.clone();}
                }
            } else if kind=="content_block_delta" && let Some((_,buffer))=pending.get_mut(&index)
                && let Some(fragment)=event.pointer("/delta/partial_json").and_then(Value::as_str) {
                if buffer.len().saturating_add(fragment.len())>MAX_ARGUMENT_BYTES {yield Err(io::Error::other("Adapted tool arguments exceed 4 MiB"));return;}
                buffer.push_str(fragment);
                continue;
            } else if kind=="content_block_stop" && let Some((adapter,buffer))=pending.remove(&index) {
                let source=if buffer.is_empty(){"{}"}else{&buffer};
                let mut input:Value=match serde_json::from_str(source) {
                    Ok(Value::Object(object))=>Value::Object(object),
                    _=>{yield Err(io::Error::other("Invalid JSON object in adapted tool arguments"));return;}
                };
                adapter.reverse(&mut input);
                yield Ok(json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":input.to_string()}}));
            } else if kind=="message_stop" && !pending.is_empty() {
                yield Err(io::Error::other("Incomplete adapted tool arguments"));return;
            }
            let terminal=matches!(kind.as_str(),"message_stop"|"error");
            yield Ok(event);
            if terminal {return;}
        }
        if !pending.is_empty() {yield Err(io::Error::other("Incomplete adapted tool arguments"));}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{compatibility::ClientProfile, transforms::tool_aliases::normalize_tools};
    use futures_util::stream;

    #[tokio::test]
    async fn malformed_truncated_and_oversized_arguments_fail() {
        let mut body = json!({"tools":[{"name":"read_file","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}]});
        let map = normalize_tools(&mut body, ClientProfile::OpenAi);
        for (fragment, stop) in [
            ("{broken".to_owned(), true),
            ("{\"file_path\":".to_owned(), false),
            ("x".repeat(MAX_ARGUMENT_BYTES + 1), true),
            ("[]".to_owned(), true),
        ] {
            let mut events = vec![
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","name":"mcp_Read","input":{}}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":fragment}}),
            ];
            if stop {
                events.push(json!({"type":"content_block_stop","index":0}));
            }
            let output: Vec<_> = restore(stream::iter(events.into_iter().map(Ok)), map.clone())
                .collect()
                .await;
            assert!(output.last().unwrap().is_err());
        }
    }
}
