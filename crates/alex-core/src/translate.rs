use serde_json::{json, Map, Value};

fn put(o: &mut Map<String, Value>, k: &str, v: &Value) {
    if !v.is_null() {
        o.insert(k.to_string(), v.clone());
    }
}

pub(crate) fn txt(c: &Value) -> String {
    match c {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn parse_args(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|_| json!({}))
}

pub fn openai_chat_to_anthropic(req: &Value) -> Value {
    let mut sys = Vec::new();
    let mut msgs = Vec::new();
    for m in req["messages"].as_array().into_iter().flatten() {
        match m["role"].as_str().unwrap_or("") {
            "system" => sys.push(txt(&m["content"])),
            "user" => msgs.push(json!({"role": "user", "content": txt(&m["content"])})),
            "assistant" => {
                let mut blocks = Vec::new();
                let t = txt(&m["content"]);
                if !t.is_empty() {
                    blocks.push(json!({"type": "text", "text": t}));
                }
                for tc in m["tool_calls"].as_array().into_iter().flatten() {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": tc["id"],
                        "name": tc["function"]["name"],
                        "input": parse_args(tc["function"]["arguments"].as_str().unwrap_or("{}")),
                    }));
                }
                msgs.push(json!({"role": "assistant", "content": blocks}));
            }
            "tool" => msgs.push(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": m["tool_call_id"],
                    "content": [{"type": "text", "text": txt(&m["content"])}],
                }],
            })),
            _ => {}
        }
    }
    let mut o = Map::new();
    put(&mut o, "model", &req["model"]);
    if !sys.is_empty() {
        o.insert("system".to_string(), Value::String(sys.join("\n\n")));
    }
    o.insert("messages".to_string(), Value::Array(msgs));
    if let Some(ts) = req["tools"].as_array() {
        let tools: Vec<Value> = ts
            .iter()
            .filter(|t| t["function"].is_object())
            .map(|t| {
                let f = &t["function"];
                let mut tool = Map::new();
                put(&mut tool, "name", &f["name"]);
                put(&mut tool, "description", &f["description"]);
                put(&mut tool, "input_schema", &f["parameters"]);
                Value::Object(tool)
            })
            .collect();
        if !tools.is_empty() {
            o.insert("tools".to_string(), Value::Array(tools));
        }
    }
    match &req["tool_choice"] {
        Value::String(s) if s == "auto" => {
            o.insert("tool_choice".to_string(), json!({"type": "auto"}));
        }
        v if v["type"] == "function" => {
            o.insert(
                "tool_choice".to_string(),
                json!({"type": "tool", "name": v["function"]["name"]}),
            );
        }
        _ => {}
    }
    let max = req["max_tokens"]
        .as_i64()
        .or_else(|| req["max_completion_tokens"].as_i64())
        .unwrap_or(8192);
    o.insert("max_tokens".to_string(), json!(max));
    put(&mut o, "temperature", &req["temperature"]);
    put(&mut o, "top_p", &req["top_p"]);
    match &req["stop"] {
        Value::String(s) => {
            o.insert("stop_sequences".to_string(), json!([s]));
        }
        Value::Array(a) => {
            o.insert("stop_sequences".to_string(), Value::Array(a.clone()));
        }
        _ => {}
    }
    put(&mut o, "stream", &req["stream"]);
    Value::Object(o)
}

pub fn openai_responses_to_anthropic(req: &Value) -> Value {
    let mut msgs = Vec::new();
    match &req["input"] {
        Value::String(s) => msgs.push(json!({"role": "user", "content": s})),
        Value::Array(items) => {
            for it in items {
                match it["type"].as_str().unwrap_or("message") {
                    "message" => {
                        let role = if it["role"] == "assistant" {
                            "assistant"
                        } else {
                            "user"
                        };
                        msgs.push(json!({"role": role, "content": txt(&it["content"])}));
                    }
                    "function_call" => msgs.push(json!({
                        "role": "assistant",
                        "content": [{
                            "type": "tool_use",
                            "id": it["call_id"],
                            "name": it["name"],
                            "input": parse_args(it["arguments"].as_str().unwrap_or("{}")),
                        }],
                    })),
                    "function_call_output" => msgs.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": it["call_id"],
                            "content": [{"type": "text", "text": txt(&it["output"])}],
                        }],
                    })),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    let mut o = Map::new();
    put(&mut o, "model", &req["model"]);
    put(&mut o, "system", &req["instructions"]);
    o.insert("messages".to_string(), Value::Array(msgs));
    if let Some(ts) = req["tools"].as_array() {
        let tools: Vec<Value> = ts
            .iter()
            .filter(|t| t["type"] == "function")
            .map(|t| {
                let mut tool = Map::new();
                put(&mut tool, "name", &t["name"]);
                put(&mut tool, "description", &t["description"]);
                put(&mut tool, "input_schema", &t["parameters"]);
                Value::Object(tool)
            })
            .collect();
        if !tools.is_empty() {
            o.insert("tools".to_string(), Value::Array(tools));
        }
    }
    o.insert(
        "max_tokens".to_string(),
        json!(req["max_output_tokens"].as_i64().unwrap_or(8192)),
    );
    put(&mut o, "temperature", &req["temperature"]);
    put(&mut o, "top_p", &req["top_p"]);
    put(&mut o, "stream", &req["stream"]);
    Value::Object(o)
}

pub fn anthropic_to_openai_responses(req: &Value) -> Value {
    let mut input = Vec::new();
    let mut sys_extra: Vec<String> = Vec::new();
    for m in req["messages"].as_array().into_iter().flatten() {
        let role = m["role"].as_str().unwrap_or("user");
        if role == "system" || role == "developer" {
            let text = txt(&m["content"]);
            if !text.is_empty() {
                sys_extra.push(text);
            }
            continue;
        }
        let part = if role == "assistant" {
            "output_text"
        } else {
            "input_text"
        };
        match &m["content"] {
            Value::String(s) => input.push(json!({
                "type": "message",
                "role": role,
                "content": [{"type": part, "text": s}],
            })),
            Value::Array(blocks) => {
                let mut parts = Vec::new();
                let mut items = Vec::new();
                for b in blocks {
                    match b["type"].as_str() {
                        Some("text") => parts.push(json!({"type": part, "text": b["text"]})),
                        Some("tool_use") => items.push(json!({
                            "type": "function_call",
                            "call_id": b["id"],
                            "name": b["name"],
                            "arguments": b["input"].to_string(),
                        })),
                        Some("tool_result") => items.push(json!({
                            "type": "function_call_output",
                            "call_id": b["tool_use_id"],
                            "output": txt(&b["content"]),
                        })),
                        _ => {}
                    }
                }
                if !parts.is_empty() {
                    input.push(json!({"type": "message", "role": role, "content": parts}));
                }
                input.extend(items);
            }
            _ => {}
        }
    }
    let mut o = Map::new();
    put(&mut o, "model", &req["model"]);
    let mut instructions = match &req["system"] {
        Value::String(s) => s.clone(),
        Value::Array(_) => txt(&req["system"]),
        _ => String::new(),
    };
    for extra in sys_extra {
        if !instructions.is_empty() {
            instructions.push_str("\n\n");
        }
        instructions.push_str(&extra);
    }
    if !instructions.is_empty() {
        o.insert("instructions".to_string(), Value::String(instructions));
    }
    o.insert("input".to_string(), Value::Array(input));
    if let Some(ts) = req["tools"].as_array() {
        let tools: Vec<Value> = ts
            .iter()
            .map(|t| {
                let mut tool = Map::new();
                tool.insert("type".to_string(), json!("function"));
                put(&mut tool, "name", &t["name"]);
                put(&mut tool, "description", &t["description"]);
                put(&mut tool, "parameters", &t["input_schema"]);
                tool.insert("strict".to_string(), json!(false));
                Value::Object(tool)
            })
            .collect();
        if !tools.is_empty() {
            o.insert("tools".to_string(), Value::Array(tools));
        }
    }
    if let Some(mt) = req["max_tokens"].as_i64() {
        o.insert("max_output_tokens".to_string(), json!(mt));
    }
    // Pi speaks Anthropic Messages to the Alexandria provider. Adaptive-thinking
    // models carry Pi's selected level in output_config.effort; preserve it when
    // translating to the OpenAI Responses API used by Codex.
    if let Some(effort) = req["output_config"]["effort"].as_str() {
        o.insert("reasoning".to_string(), json!({"effort": effort}));
    }
    put(&mut o, "stream", &req["stream"]);
    Value::Object(o)
}

/// Anthropic Messages request → OpenAI chat completions (pivot → chat).
/// Drops `thinking` / budget fields (no chat equivalent).
pub fn anthropic_to_openai_chat(req: &Value) -> Value {
    let mut msgs = Vec::new();
    let system = match &req["system"] {
        Value::String(s) => s.clone(),
        Value::Array(_) => txt(&req["system"]),
        _ => String::new(),
    };
    if !system.is_empty() {
        msgs.push(json!({"role": "system", "content": system}));
    }
    for m in req["messages"].as_array().into_iter().flatten() {
        let role = m["role"].as_str().unwrap_or("user");
        if role == "system" || role == "developer" {
            let text = txt(&m["content"]);
            if !text.is_empty() {
                msgs.push(json!({"role": "system", "content": text}));
            }
            continue;
        }
        match &m["content"] {
            Value::String(s) => msgs.push(json!({"role": role, "content": s})),
            Value::Array(blocks) => {
                if role == "assistant" {
                    let mut texts = Vec::new();
                    let mut calls = Vec::new();
                    for b in blocks {
                        match b["type"].as_str() {
                            Some("text") => {
                                texts.push(b["text"].as_str().unwrap_or("").to_string());
                            }
                            Some("tool_use") => calls.push(json!({
                                "id": b["id"],
                                "type": "function",
                                "function": {
                                    "name": b["name"],
                                    "arguments": b["input"].to_string(),
                                },
                            })),
                            _ => {}
                        }
                    }
                    let content = if texts.is_empty() {
                        Value::Null
                    } else {
                        Value::String(texts.join(""))
                    };
                    let mut msg = json!({"role": "assistant", "content": content});
                    if !calls.is_empty() {
                        msg["tool_calls"] = Value::Array(calls);
                    }
                    msgs.push(msg);
                } else {
                    // user (or other): text stays as user; tool_result → role:tool
                    let mut text_parts = Vec::new();
                    for b in blocks {
                        match b["type"].as_str() {
                            Some("text") => {
                                if let Some(t) = b["text"].as_str() {
                                    if !t.is_empty() {
                                        text_parts.push(t.to_string());
                                    }
                                }
                            }
                            Some("tool_result") => {
                                msgs.push(json!({
                                    "role": "tool",
                                    "tool_call_id": b["tool_use_id"],
                                    "content": txt(&b["content"]),
                                }));
                            }
                            _ => {}
                        }
                    }
                    if !text_parts.is_empty() {
                        msgs.push(json!({
                            "role": "user",
                            "content": text_parts.join("\n"),
                        }));
                    }
                }
            }
            _ => {}
        }
    }
    let mut o = Map::new();
    put(&mut o, "model", &req["model"]);
    o.insert("messages".to_string(), Value::Array(msgs));
    if let Some(ts) = req["tools"].as_array() {
        let tools: Vec<Value> = ts
            .iter()
            .map(|t| {
                let mut f = Map::new();
                put(&mut f, "name", &t["name"]);
                put(&mut f, "description", &t["description"]);
                f.insert(
                    "parameters".to_string(),
                    if t["input_schema"].is_object() {
                        t["input_schema"].clone()
                    } else {
                        json!({"type": "object"})
                    },
                );
                json!({"type": "function", "function": Value::Object(f)})
            })
            .collect();
        if !tools.is_empty() {
            o.insert("tools".to_string(), Value::Array(tools));
        }
    }
    match &req["tool_choice"] {
        Value::Object(tc) => match tc.get("type").and_then(|v| v.as_str()) {
            Some("auto") => {
                o.insert("tool_choice".to_string(), json!("auto"));
            }
            Some("any") => {
                o.insert("tool_choice".to_string(), json!("required"));
            }
            Some("none") => {
                o.insert("tool_choice".to_string(), json!("none"));
            }
            Some("tool") => {
                o.insert(
                    "tool_choice".to_string(),
                    json!({
                        "type": "function",
                        "function": {"name": tc.get("name").cloned().unwrap_or(json!(""))},
                    }),
                );
            }
            _ => {}
        },
        _ => {}
    }
    if let Some(mt) = req["max_tokens"].as_i64() {
        o.insert("max_tokens".to_string(), json!(mt));
    }
    put(&mut o, "temperature", &req["temperature"]);
    put(&mut o, "top_p", &req["top_p"]);
    match &req["stop_sequences"] {
        Value::Array(a) if !a.is_empty() => {
            o.insert("stop".to_string(), Value::Array(a.clone()));
        }
        _ => {}
    }
    put(&mut o, "stream", &req["stream"]);
    // thinking / budget_tokens intentionally dropped — no chat equivalent
    Value::Object(o)
}

fn stop_to_finish(stop: Option<&str>) -> &'static str {
    match stop {
        Some("max_tokens") => "length",
        Some("tool_use") => "tool_calls",
        _ => "stop",
    }
}

fn finish_to_stop(finish: Option<&str>) -> &'static str {
    match finish {
        Some("length") => "max_tokens",
        Some("tool_calls") => "tool_use",
        _ => "end_turn",
    }
}

pub fn anthropic_response_to_openai_chat(resp: &Value, model: &str) -> Value {
    let mut texts = Vec::new();
    let mut calls = Vec::new();
    for b in resp["content"].as_array().into_iter().flatten() {
        match b["type"].as_str() {
            Some("text") => texts.push(b["text"].as_str().unwrap_or("").to_string()),
            Some("tool_use") => calls.push(json!({
                "id": b["id"],
                "type": "function",
                "function": {"name": b["name"], "arguments": b["input"].to_string()},
            })),
            _ => {}
        }
    }
    let content = if texts.is_empty() {
        Value::Null
    } else {
        Value::String(texts.join(""))
    };
    let mut msg = json!({"role": "assistant", "content": content});
    if !calls.is_empty() {
        msg["tool_calls"] = Value::Array(calls);
    }
    let u = &resp["usage"];
    let pt = u["input_tokens"].as_i64().unwrap_or(0);
    let ct = u["output_tokens"].as_i64().unwrap_or(0);
    json!({
        "id": format!("chatcmpl-{}", resp["id"].as_str().unwrap_or("")),
        "object": "chat.completion",
        "created": 0,
        "model": model,
        "choices": [{
            "index": 0,
            "message": msg,
            "finish_reason": stop_to_finish(resp["stop_reason"].as_str()),
        }],
        "usage": {
            "prompt_tokens": pt,
            "completion_tokens": ct,
            "total_tokens": pt + ct,
            "prompt_tokens_details": {
                "cached_tokens": u["cache_read_input_tokens"].as_i64().unwrap_or(0),
            },
        },
    })
}

/// OpenAI chat completion response → Anthropic Messages shape (chat → pivot).
pub fn openai_chat_response_to_anthropic(resp: &Value, model: &str) -> Value {
    let msg = &resp["choices"][0]["message"];
    let mut content = Vec::new();
    if let Some(t) = msg["content"].as_str() {
        if !t.is_empty() {
            content.push(json!({"type": "text", "text": t}));
        }
    }
    for tc in msg["tool_calls"].as_array().into_iter().flatten() {
        content.push(json!({
            "type": "tool_use",
            "id": tc["id"],
            "name": tc["function"]["name"],
            "input": parse_args(tc["function"]["arguments"].as_str().unwrap_or("{}")),
        }));
    }
    let u = &resp["usage"];
    let pt = u["prompt_tokens"].as_i64().unwrap_or(0);
    let ct = u["completion_tokens"].as_i64().unwrap_or(0);
    let cached = u["prompt_tokens_details"]["cached_tokens"]
        .as_i64()
        .unwrap_or(0);
    let id = resp["id"].as_str().unwrap_or("");
    let msg_id = id
        .strip_prefix("chatcmpl-")
        .unwrap_or(id);
    json!({
        "id": if msg_id.is_empty() { "msg_chat".to_string() } else { format!("msg_{msg_id}") },
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": finish_to_stop(resp["choices"][0]["finish_reason"].as_str()),
        "stop_sequence": null,
        "usage": {
            "input_tokens": pt,
            "output_tokens": ct,
            "cache_read_input_tokens": cached,
        },
    })
}

pub fn anthropic_response_to_openai_responses(resp: &Value, model: &str) -> Value {
    let id = resp["id"].as_str().unwrap_or("");
    let mut output = Vec::new();
    for b in resp["content"].as_array().into_iter().flatten() {
        match b["type"].as_str() {
            Some("text") => output.push(json!({
                "type": "message",
                "id": format!("msg_{id}"),
                "role": "assistant",
                "status": "completed",
                "content": [{"type": "output_text", "text": b["text"], "annotations": []}],
            })),
            Some("tool_use") => output.push(json!({
                "type": "function_call",
                "id": b["id"],
                "call_id": b["id"],
                "name": b["name"],
                "arguments": b["input"].to_string(),
                "status": "completed",
            })),
            _ => {}
        }
    }
    let status = if resp["stop_reason"] == "max_tokens" {
        "incomplete"
    } else {
        "completed"
    };
    let u = &resp["usage"];
    let it = u["input_tokens"].as_i64().unwrap_or(0);
    let ot = u["output_tokens"].as_i64().unwrap_or(0);
    json!({
        "id": format!("resp_{id}"),
        "object": "response",
        "status": status,
        "model": model,
        "output": output,
        "usage": {
            "input_tokens": it,
            "output_tokens": ot,
            "total_tokens": it + ot,
            "input_tokens_details": {
                "cached_tokens": u["cache_read_input_tokens"].as_i64().unwrap_or(0),
            },
            "output_tokens_details": {"reasoning_tokens": 0},
        },
    })
}

pub fn responses_final_to_anthropic(resp: &Value, model: &str) -> Value {
    let mut content = Vec::new();
    let mut has_call = false;
    for it in resp["output"].as_array().into_iter().flatten() {
        match it["type"].as_str() {
            Some("message") => {
                for p in it["content"].as_array().into_iter().flatten() {
                    if p["type"] == "output_text" {
                        content.push(json!({"type": "text", "text": p["text"]}));
                    }
                }
            }
            Some("function_call") => {
                has_call = true;
                content.push(json!({
                    "type": "tool_use",
                    "id": it["call_id"],
                    "name": it["name"],
                    "input": parse_args(it["arguments"].as_str().unwrap_or("{}")),
                }));
            }
            _ => {}
        }
    }
    let stop = if resp["status"] == "incomplete" {
        "max_tokens"
    } else if has_call {
        "tool_use"
    } else {
        "end_turn"
    };
    let u = &resp["usage"];
    json!({
        "id": format!("msg_{}", resp["id"].as_str().unwrap_or("")),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop,
        "stop_sequence": null,
        "usage": {
            "input_tokens": u["input_tokens"].as_i64().unwrap_or(0),
            "output_tokens": u["output_tokens"].as_i64().unwrap_or(0),
            "cache_read_input_tokens": u["input_tokens_details"]["cached_tokens"].as_i64().unwrap_or(0),
        },
    })
}

pub fn responses_final_to_openai_chat(resp: &Value, model: &str) -> Value {
    let mut texts = Vec::new();
    let mut calls = Vec::new();
    for it in resp["output"].as_array().into_iter().flatten() {
        match it["type"].as_str() {
            Some("message") => {
                for p in it["content"].as_array().into_iter().flatten() {
                    if p["type"] == "output_text" {
                        texts.push(p["text"].as_str().unwrap_or("").to_string());
                    }
                }
            }
            Some("function_call") => calls.push(json!({
                "id": it["call_id"],
                "type": "function",
                "function": {"name": it["name"], "arguments": it["arguments"]},
            })),
            _ => {}
        }
    }
    let content = if texts.is_empty() {
        Value::Null
    } else {
        Value::String(texts.join(""))
    };
    let mut msg = json!({"role": "assistant", "content": content});
    let finish = if resp["status"] == "incomplete" {
        "length"
    } else if calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    if !calls.is_empty() {
        msg["tool_calls"] = Value::Array(calls);
    }
    let u = &resp["usage"];
    let pt = u["input_tokens"].as_i64().unwrap_or(0);
    let ct = u["output_tokens"].as_i64().unwrap_or(0);
    json!({
        "id": format!("chatcmpl-{}", resp["id"].as_str().unwrap_or("")),
        "object": "chat.completion",
        "created": 0,
        "model": model,
        "choices": [{"index": 0, "message": msg, "finish_reason": finish}],
        "usage": {
            "prompt_tokens": pt,
            "completion_tokens": ct,
            "total_tokens": pt + ct,
            "prompt_tokens_details": {
                "cached_tokens": u["input_tokens_details"]["cached_tokens"].as_i64().unwrap_or(0),
            },
        },
    })
}

fn sse_datas(sse: &str) -> impl Iterator<Item = Value> + '_ {
    sse.lines().filter_map(|l| {
        let d = l.strip_prefix("data:")?.trim();
        if d.is_empty() || d == "[DONE]" {
            return None;
        }
        serde_json::from_str(d).ok()
    })
}

pub fn parse_anthropic_sse_to_message(sse: &str) -> Option<Value> {
    let mut msg: Option<Value> = None;
    let mut blocks: Vec<Value> = Vec::new();
    let mut partials: Vec<String> = Vec::new();
    for v in sse_datas(sse) {
        match v["type"].as_str() {
            Some("message_start") => {
                if v["message"].is_object() {
                    msg = Some(v["message"].clone());
                }
            }
            Some("content_block_start") => {
                let i = v["index"].as_u64().unwrap_or(blocks.len() as u64) as usize;
                while blocks.len() <= i {
                    blocks.push(Value::Null);
                    partials.push(String::new());
                }
                blocks[i] = v["content_block"].clone();
                partials[i] = String::new();
            }
            Some("content_block_delta") => {
                let i = v["index"].as_u64().unwrap_or(0) as usize;
                if i >= blocks.len() {
                    continue;
                }
                let d = &v["delta"];
                match d["type"].as_str() {
                    Some("text_delta") => {
                        let t = format!(
                            "{}{}",
                            blocks[i]["text"].as_str().unwrap_or(""),
                            d["text"].as_str().unwrap_or("")
                        );
                        blocks[i]["text"] = json!(t);
                    }
                    Some("input_json_delta") => {
                        partials[i].push_str(d["partial_json"].as_str().unwrap_or(""));
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                let i = v["index"].as_u64().unwrap_or(0) as usize;
                if i < blocks.len() && blocks[i]["type"] == "tool_use" && !partials[i].is_empty() {
                    blocks[i]["input"] = parse_args(&partials[i]);
                }
            }
            Some("message_delta") => {
                let Some(m) = msg.as_mut() else { continue };
                for k in ["stop_reason", "stop_sequence"] {
                    if !v["delta"][k].is_null() {
                        m[k] = v["delta"][k].clone();
                    }
                }
                if let Some(uo) = v["usage"].as_object() {
                    if !m["usage"].is_object() {
                        m["usage"] = json!({});
                    }
                    for (k, val) in uo {
                        m["usage"][k.as_str()] = val.clone();
                    }
                }
            }
            _ => {}
        }
    }
    let mut m = msg?;
    m["content"] = Value::Array(blocks.into_iter().filter(|b| !b.is_null()).collect());
    Some(m)
}

pub fn parse_responses_sse_final(sse: &str) -> Option<Value> {
    let mut last = None;
    let mut items: Vec<Value> = Vec::new();
    for v in sse_datas(sse) {
        match v["type"].as_str() {
            Some("response.completed" | "response.incomplete" | "response.failed") => {
                last = Some(v["response"].clone());
            }
            Some("response.output_item.done") => {
                if v["item"].is_object() {
                    items.push(v["item"].clone());
                }
            }
            _ => {}
        }
    }
    let mut resp = last?;
    if resp["output"]
        .as_array()
        .map(|a| a.is_empty())
        .unwrap_or(true)
        && !items.is_empty()
    {
        resp["output"] = Value::Array(items);
    }
    Some(resp)
}

/// Reassemble OpenAI chat.completion.chunk SSE into a final chat.completion object.
pub fn parse_openai_chat_sse_final(sse: &str) -> Option<Value> {
    let mut id = String::new();
    let mut model = String::new();
    let mut content = String::new();
    let mut tool_calls: Vec<(String, String, String)> = Vec::new(); // id, name, args
    let mut finish_reason = Value::Null;
    let mut usage = Value::Null;
    let mut saw_chunk = false;
    for v in sse_datas(sse) {
        if v["object"] == "chat.completion" && v["choices"][0]["message"].is_object() {
            // non-chunk full object in a data: line
            return Some(v);
        }
        if !v["choices"].is_array() && v.get("usage").is_none() {
            continue;
        }
        saw_chunk = true;
        if let Some(s) = v["id"].as_str() {
            if !s.is_empty() {
                id = s.to_string();
            }
        }
        if let Some(s) = v["model"].as_str() {
            if !s.is_empty() {
                model = s.to_string();
            }
        }
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            usage = u.clone();
        }
        let choice = &v["choices"][0];
        if !choice["finish_reason"].is_null() {
            finish_reason = choice["finish_reason"].clone();
        }
        let delta = &choice["delta"];
        if let Some(c) = delta["content"].as_str() {
            content.push_str(c);
        }
        for tc in delta["tool_calls"].as_array().into_iter().flatten() {
            let idx = tc["index"].as_u64().unwrap_or(0) as usize;
            while tool_calls.len() <= idx {
                tool_calls.push((String::new(), String::new(), String::new()));
            }
            if let Some(tc_id) = tc["id"].as_str() {
                tool_calls[idx].0.push_str(tc_id);
            }
            if let Some(n) = tc["function"]["name"].as_str() {
                tool_calls[idx].1.push_str(n);
            }
            if let Some(a) = tc["function"]["arguments"].as_str() {
                tool_calls[idx].2.push_str(a);
            }
        }
        // some providers put the full message on the last chunk instead of delta
        if let Some(m) = choice.get("message") {
            if let Some(c) = m["content"].as_str() {
                if content.is_empty() {
                    content.push_str(c);
                }
            }
            if tool_calls.is_empty() {
                for tc in m["tool_calls"].as_array().into_iter().flatten() {
                    tool_calls.push((
                        tc["id"].as_str().unwrap_or("").to_string(),
                        tc["function"]["name"].as_str().unwrap_or("").to_string(),
                        tc["function"]["arguments"].as_str().unwrap_or("").to_string(),
                    ));
                }
            }
        }
    }
    if !saw_chunk {
        return None;
    }
    let content_val = if content.is_empty() {
        Value::Null
    } else {
        Value::String(content)
    };
    let mut msg = json!({"role": "assistant", "content": content_val});
    if !tool_calls.is_empty() {
        let tcs: Vec<Value> = tool_calls
            .into_iter()
            .filter(|(i, n, _)| !i.is_empty() || !n.is_empty())
            .map(|(i, n, a)| {
                json!({
                    "id": i,
                    "type": "function",
                    "function": {"name": n, "arguments": a},
                })
            })
            .collect();
        if !tcs.is_empty() {
            msg["tool_calls"] = Value::Array(tcs);
        }
    }
    let mut out = json!({
        "id": id,
        "object": "chat.completion",
        "created": 0,
        "model": model,
        "choices": [{
            "index": 0,
            "message": msg,
            "finish_reason": finish_reason,
        }],
    });
    if usage.is_object() {
        out["usage"] = usage;
    }
    Some(out)
}

pub fn synth_openai_chat_sse(chat_resp: &Value) -> String {
    let chunk = |delta: Value, finish: Value, usage: Option<&Value>| {
        let mut c = json!({
            "id": chat_resp["id"],
            "object": "chat.completion.chunk",
            "created": 0,
            "model": chat_resp["model"],
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        });
        if let Some(u) = usage {
            c["usage"] = u.clone();
        }
        format!("data: {c}\n\n")
    };
    let msg = &chat_resp["choices"][0]["message"];
    let mut out = chunk(json!({"role": "assistant"}), Value::Null, None);
    if let Some(t) = msg["content"].as_str() {
        out.push_str(&chunk(json!({"content": t}), Value::Null, None));
    }
    if let Some(tcs) = msg["tool_calls"].as_array() {
        let tcs: Vec<Value> = tcs
            .iter()
            .enumerate()
            .map(|(i, tc)| {
                let mut tc = tc.clone();
                tc["index"] = json!(i);
                tc
            })
            .collect();
        out.push_str(&chunk(json!({"tool_calls": tcs}), Value::Null, None));
    }
    let usage = chat_resp["usage"]
        .is_object()
        .then_some(&chat_resp["usage"]);
    out.push_str(&chunk(
        json!({}),
        chat_resp["choices"][0]["finish_reason"].clone(),
        usage,
    ));
    out.push_str("data: [DONE]\n\n");
    out
}

fn sse_event(name: &str, data: Value) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}

pub fn synth_openai_responses_sse(responses_resp: &Value) -> String {
    let mut created = responses_resp.clone();
    created["status"] = json!("in_progress");
    let mut out = sse_event(
        "response.created",
        json!({"type": "response.created", "response": created}),
    );
    for (i, it) in responses_resp["output"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        out.push_str(&sse_event(
            "response.output_item.added",
            json!({"type": "response.output_item.added", "output_index": i, "item": it}),
        ));
        if it["type"] == "message" {
            let text = txt(&it["content"]);
            out.push_str(&sse_event(
                "response.output_text.delta",
                json!({
                    "type": "response.output_text.delta",
                    "item_id": it["id"],
                    "output_index": i,
                    "content_index": 0,
                    "delta": text,
                }),
            ));
            out.push_str(&sse_event(
                "response.output_text.done",
                json!({
                    "type": "response.output_text.done",
                    "item_id": it["id"],
                    "output_index": i,
                    "content_index": 0,
                    "text": text,
                }),
            ));
        }
        // Codex consumes completed output items, including function calls, from
        // this event. `response.completed` alone does not dispatch its tool loop.
        out.push_str(&sse_event(
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "output_index": i,
                "item": it,
            }),
        ));
    }
    out.push_str(&sse_event(
        "response.completed",
        json!({"type": "response.completed", "response": responses_resp}),
    ));
    out
}

pub fn synth_anthropic_sse(anthropic_resp: &Value) -> String {
    let mut start = anthropic_resp.clone();
    start["content"] = json!([]);
    start["stop_reason"] = Value::Null;
    start["stop_sequence"] = Value::Null;
    start["usage"] = json!({
        "input_tokens": anthropic_resp["usage"]["input_tokens"].as_i64().unwrap_or(0),
        "output_tokens": 0,
    });
    let mut out = sse_event(
        "message_start",
        json!({"type": "message_start", "message": start}),
    );
    for (i, b) in anthropic_resp["content"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        match b["type"].as_str() {
            Some("text") => {
                out.push_str(&sse_event(
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": i,
                        "content_block": {"type": "text", "text": ""},
                    }),
                ));
                out.push_str(&sse_event(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": i,
                        "delta": {"type": "text_delta", "text": b["text"]},
                    }),
                ));
            }
            Some("tool_use") => {
                out.push_str(&sse_event(
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": i,
                        "content_block": {"type": "tool_use", "id": b["id"], "name": b["name"], "input": {}},
                    }),
                ));
                out.push_str(&sse_event(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": i,
                        "delta": {"type": "input_json_delta", "partial_json": b["input"].to_string()},
                    }),
                ));
            }
            _ => continue,
        }
        out.push_str(&sse_event(
            "content_block_stop",
            json!({"type": "content_block_stop", "index": i}),
        ));
    }
    out.push_str(&sse_event(
        "message_delta",
        json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": anthropic_resp["stop_reason"],
                "stop_sequence": anthropic_resp["stop_sequence"],
            },
            "usage": {
                "output_tokens": anthropic_resp["usage"]["output_tokens"].as_i64().unwrap_or(0),
            },
        }),
    ));
    out.push_str(&sse_event("message_stop", json!({"type": "message_stop"})));
    out
}

fn tool_result_snip(text: &str) -> String {
    let head: String = text.chars().take(200).collect();
    format!("[tool result] {head}")
}

pub fn last_user_text(format_str: &str, req: &Value) -> Option<String> {
    match format_str {
        "anthropic" => {
            for m in req["messages"].as_array().into_iter().flatten().rev() {
                if m["role"] != "user" {
                    continue;
                }
                match &m["content"] {
                    Value::String(s) if !s.is_empty() => return Some(s.clone()),
                    Value::Array(blocks) => {
                        let text = blocks
                            .iter()
                            .filter(|b| b["type"] == "text")
                            .filter_map(|b| b["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n");
                        if !text.is_empty() {
                            return Some(text);
                        }
                        if let Some(tr) = blocks.iter().find(|b| b["type"] == "tool_result") {
                            return Some(tool_result_snip(&txt(&tr["content"])));
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        "openai-chat" => {
            for m in req["messages"].as_array().into_iter().flatten().rev() {
                match m["role"].as_str() {
                    Some("user") => {
                        let t = txt(&m["content"]);
                        if !t.is_empty() {
                            return Some(t);
                        }
                    }
                    Some("tool") => return Some(tool_result_snip(&txt(&m["content"]))),
                    _ => {}
                }
            }
            None
        }
        "openai-responses" => {
            if let Some(s) = req["input"].as_str() {
                return (!s.is_empty()).then(|| s.to_string());
            }
            for it in req["input"].as_array().into_iter().flatten().rev() {
                match it["type"].as_str().unwrap_or("message") {
                    "message" if it["role"] == "user" => {
                        let t = match &it["content"] {
                            Value::String(s) => s.clone(),
                            Value::Array(parts) => parts
                                .iter()
                                .filter(|p| p["type"] == "input_text")
                                .filter_map(|p| p["text"].as_str())
                                .collect::<Vec<_>>()
                                .join("\n"),
                            _ => String::new(),
                        };
                        if !t.is_empty() {
                            return Some(t);
                        }
                    }
                    "function_call_output" => return Some(tool_result_snip(&txt(&it["output"]))),
                    _ => {}
                }
            }
            None
        }
        "gemini" => {
            for c in req["contents"].as_array().into_iter().flatten().rev() {
                if c["role"].as_str().unwrap_or("user") != "user" {
                    continue;
                }
                let text = gemini_parts_text(&c["parts"]);
                if !text.is_empty() {
                    return Some(text);
                }
                if let Some(fr) = c["parts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|p| p["functionResponse"].is_object())
                {
                    return Some(tool_result_snip(
                        &fr["functionResponse"]["response"].to_string(),
                    ));
                }
            }
            None
        }
        _ => None,
    }
}

fn anthropic_message_text(msg: &Value) -> Option<String> {
    let parts: Vec<&str> = msg["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    (!parts.is_empty()).then(|| parts.join(""))
}

fn responses_output_text(resp: &Value) -> Option<String> {
    let mut out = String::new();
    for it in resp["output"].as_array().into_iter().flatten() {
        if it["type"] != "message" {
            continue;
        }
        for p in it["content"].as_array().into_iter().flatten() {
            if p["type"] == "output_text" {
                out.push_str(p["text"].as_str().unwrap_or(""));
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

fn openai_chat_sse_text(sse: &str) -> Option<String> {
    let mut out = String::new();
    for v in sse_datas(sse) {
        if let Some(c) = v["choices"][0]["delta"]["content"].as_str() {
            out.push_str(c);
        }
    }
    (!out.is_empty()).then_some(out)
}

fn tool_call_json(name: &Value, args: &Value) -> Value {
    let arguments = match args {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    json!({"name": name, "arguments": arguments})
}

pub fn assistant_tool_calls(upstream_format: &str, resp_text: &str) -> Vec<Value> {
    let trimmed = resp_text.trim_start();
    let is_sse = trimmed.starts_with("event:") || trimmed.starts_with("data:");
    match upstream_format {
        "anthropic" => {
            let msg = if is_sse {
                parse_anthropic_sse_to_message(resp_text)
            } else {
                serde_json::from_str(resp_text).ok()
            };
            msg.map(|m| {
                m["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"] == "tool_use")
                    .map(|b| tool_call_json(&b["name"], &b["input"]))
                    .collect()
            })
            .unwrap_or_default()
        }
        "openai-chat" => {
            if is_sse {
                let mut calls: Vec<(String, String)> = Vec::new();
                for v in sse_datas(resp_text) {
                    for tc in v["choices"][0]["delta"]["tool_calls"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                        while calls.len() <= idx {
                            calls.push((String::new(), String::new()));
                        }
                        if let Some(n) = tc["function"]["name"].as_str() {
                            calls[idx].0.push_str(n);
                        }
                        if let Some(a) = tc["function"]["arguments"].as_str() {
                            calls[idx].1.push_str(a);
                        }
                    }
                }
                calls
                    .into_iter()
                    .filter(|(n, _)| !n.is_empty())
                    .map(|(n, a)| json!({"name": n, "arguments": a}))
                    .collect()
            } else {
                serde_json::from_str::<Value>(resp_text)
                    .ok()
                    .map(|v| {
                        v["choices"][0]["message"]["tool_calls"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|tc| {
                                tool_call_json(
                                    &tc["function"]["name"],
                                    &tc["function"]["arguments"],
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
        }
        "openai-responses" => {
            let resp = if is_sse {
                parse_responses_sse_final(resp_text)
            } else {
                serde_json::from_str(resp_text).ok()
            };
            resp.map(|r| {
                r["output"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|it| it["type"] == "function_call")
                    .map(|it| tool_call_json(&it["name"], &it["arguments"]))
                    .collect()
            })
            .unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

pub fn assistant_reply_text(upstream_format: &str, resp_text: &str) -> Option<String> {
    let trimmed = resp_text.trim_start();
    let is_sse = trimmed.starts_with("event:") || trimmed.starts_with("data:");
    match upstream_format {
        "anthropic" => {
            let msg = if is_sse {
                parse_anthropic_sse_to_message(resp_text)?
            } else {
                serde_json::from_str(resp_text).ok()?
            };
            anthropic_message_text(&msg)
        }
        "openai-chat" => {
            if is_sse {
                openai_chat_sse_text(resp_text)
            } else {
                let v: Value = serde_json::from_str(resp_text).ok()?;
                v["choices"][0]["message"]["content"]
                    .as_str()
                    .map(String::from)
            }
        }
        "openai-responses" => {
            let resp = if is_sse {
                parse_responses_sse_final(resp_text)?
            } else {
                serde_json::from_str(resp_text).ok()?
            };
            responses_output_text(&resp)
        }
        "gemini" => {
            if is_sse {
                let mut out = String::new();
                for v in sse_datas(resp_text) {
                    out.push_str(&gemini_parts_text(&v["candidates"][0]["content"]["parts"]));
                }
                (!out.is_empty()).then_some(out)
            } else {
                let v: Value = serde_json::from_str(resp_text).ok()?;
                let text = gemini_parts_text(&v["candidates"][0]["content"]["parts"]);
                (!text.is_empty()).then_some(text)
            }
        }
        _ => None,
    }
}

pub(crate) fn gemini_parts_text(parts: &Value) -> String {
    parts
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn gemini_to_anthropic(req: &Value) -> Value {
    let mut msgs = Vec::new();
    let mut call_ids: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut call_counter = 0usize;
    for content in req["contents"].as_array().into_iter().flatten() {
        let role = content["role"].as_str().unwrap_or("user");
        let mut blocks = Vec::new();
        for part in content["parts"].as_array().into_iter().flatten() {
            if let Some(t) = part["text"].as_str() {
                if !t.is_empty() {
                    blocks.push(json!({"type": "text", "text": t}));
                }
            } else if part["functionCall"].is_object() {
                call_counter += 1;
                let name = part["functionCall"]["name"].as_str().unwrap_or("");
                let id = format!("toolu_gemini_{call_counter}");
                call_ids.insert(name.to_string(), id.clone());
                blocks.push(json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": if part["functionCall"]["args"].is_object() {
                        part["functionCall"]["args"].clone()
                    } else {
                        json!({})
                    },
                }));
            } else if part["functionResponse"].is_object() {
                let name = part["functionResponse"]["name"].as_str().unwrap_or("");
                let id = call_ids
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| format!("toolu_gemini_{name}"));
                let payload = &part["functionResponse"]["response"];
                let text = match payload {
                    Value::String(s) => s.clone(),
                    v if v.is_null() => String::new(),
                    v => v.to_string(),
                };
                blocks.push(json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": [{"type": "text", "text": text}],
                }));
            }
        }
        if blocks.is_empty() {
            continue;
        }
        let a_role = if role == "model" { "assistant" } else { "user" };
        msgs.push(json!({"role": a_role, "content": blocks}));
    }
    let mut o = Map::new();
    put(&mut o, "model", &req["model"]);
    let system = gemini_parts_text(&req["systemInstruction"]["parts"]);
    if !system.trim().is_empty() {
        o.insert("system".to_string(), Value::String(system));
    }
    o.insert("messages".to_string(), Value::Array(msgs));
    let tools: Vec<Value> = req["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|t| {
            t["functionDeclarations"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|fd| {
            let mut tool = Map::new();
            put(&mut tool, "name", &fd["name"]);
            put(&mut tool, "description", &fd["description"]);
            if fd["parameters"].is_object() {
                tool.insert("input_schema".to_string(), fd["parameters"].clone());
            } else {
                tool.insert("input_schema".to_string(), json!({"type": "object"}));
            }
            Value::Object(tool)
        })
        .collect();
    if !tools.is_empty() {
        o.insert("tools".to_string(), Value::Array(tools));
    }
    match req["toolConfig"]["functionCallingConfig"]["mode"].as_str() {
        Some("ANY") => {
            o.insert("tool_choice".to_string(), json!({"type": "any"}));
        }
        Some("AUTO") => {
            o.insert("tool_choice".to_string(), json!({"type": "auto"}));
        }
        _ => {}
    }
    let g = &req["generationConfig"];
    let max = g["maxOutputTokens"].as_i64().unwrap_or(8192);
    o.insert("max_tokens".to_string(), json!(max));
    put(&mut o, "temperature", &g["temperature"]);
    put(&mut o, "top_p", &g["topP"]);
    if let Some(stops) = g["stopSequences"].as_array() {
        if !stops.is_empty() {
            o.insert("stop_sequences".to_string(), Value::Array(stops.clone()));
        }
    }
    Value::Object(o)
}

fn stop_to_gemini_finish(stop: Option<&str>) -> &'static str {
    match stop {
        Some("max_tokens") => "MAX_TOKENS",
        _ => "STOP",
    }
}

pub fn anthropic_response_to_gemini(resp: &Value, model: &str) -> Value {
    let mut parts = Vec::new();
    for b in resp["content"].as_array().into_iter().flatten() {
        match b["type"].as_str() {
            Some("text") => {
                parts.push(json!({"text": b["text"].as_str().unwrap_or("")}));
            }
            Some("tool_use") => {
                parts.push(json!({
                    "functionCall": {"name": b["name"], "args": b["input"].clone()},
                }));
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        parts.push(json!({"text": ""}));
    }
    let u = &resp["usage"];
    let pt = u["input_tokens"].as_i64().unwrap_or(0)
        + u["cache_read_input_tokens"].as_i64().unwrap_or(0);
    let ct = u["output_tokens"].as_i64().unwrap_or(0);
    json!({
        "candidates": [{
            "content": {"role": "model", "parts": parts},
            "finishReason": stop_to_gemini_finish(resp["stop_reason"].as_str()),
            "index": 0,
        }],
        "usageMetadata": {
            "promptTokenCount": pt,
            "candidatesTokenCount": ct,
            "totalTokenCount": pt + ct,
            "cachedContentTokenCount": u["cache_read_input_tokens"].as_i64().unwrap_or(0),
        },
        "modelVersion": model,
    })
}

pub fn anthropic_to_gemini_request(req: &Value) -> Value {
    let mut contents = Vec::new();
    let mut tool_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for m in req["messages"].as_array().into_iter().flatten() {
        let role = if m["role"] == "assistant" {
            "model"
        } else {
            "user"
        };
        let mut parts = Vec::new();
        match &m["content"] {
            Value::String(s) => {
                if !s.is_empty() {
                    parts.push(json!({"text": s}));
                }
            }
            Value::Array(blocks) => {
                for b in blocks {
                    match b["type"].as_str() {
                        Some("text") => {
                            parts.push(json!({"text": b["text"].as_str().unwrap_or("")}));
                        }
                        Some("tool_use") => {
                            let id = b["id"].as_str().unwrap_or("").to_string();
                            let name = b["name"].as_str().unwrap_or("").to_string();
                            tool_names.insert(id, name.clone());
                            parts.push(json!({
                                "functionCall": {"name": name, "args": b["input"].clone()},
                            }));
                        }
                        Some("tool_result") => {
                            let id = b["tool_use_id"].as_str().unwrap_or("");
                            let name = tool_names
                                .get(id)
                                .cloned()
                                .unwrap_or_else(|| id.to_string());
                            parts.push(json!({
                                "functionResponse": {
                                    "name": name,
                                    "response": {"result": txt(&b["content"])},
                                },
                            }));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if !parts.is_empty() {
            contents.push(json!({"role": role, "parts": parts}));
        }
    }
    let mut o = Map::new();
    o.insert("contents".to_string(), Value::Array(contents));
    let system = txt(&req["system"]);
    if !system.is_empty() {
        o.insert(
            "systemInstruction".to_string(),
            json!({"parts": [{"text": system}]}),
        );
    }
    let decls: Vec<Value> = req["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            json!({
                "name": t["name"],
                "description": t["description"],
                "parameters": if t["input_schema"].is_object() {
                    t["input_schema"].clone()
                } else {
                    json!({"type": "object"})
                },
            })
        })
        .collect();
    if !decls.is_empty() {
        o.insert(
            "tools".to_string(),
            json!([{"functionDeclarations": decls}]),
        );
    }
    let mut g = Map::new();
    put(&mut g, "temperature", &req["temperature"]);
    put(&mut g, "topP", &req["top_p"]);
    put(&mut g, "maxOutputTokens", &req["max_tokens"]);
    if let Some(stops) = req["stop_sequences"].as_array() {
        if !stops.is_empty() {
            g.insert("stopSequences".to_string(), Value::Array(stops.clone()));
        }
    }
    if !g.is_empty() {
        o.insert("generationConfig".to_string(), Value::Object(g));
    }
    Value::Object(o)
}

pub fn gemini_response_to_anthropic(resp: &Value, model: &str) -> Value {
    let mut content = Vec::new();
    let mut saw_tool = false;
    let mut call_counter = 0usize;
    for part in resp["candidates"][0]["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if let Some(t) = part["text"].as_str() {
            if !t.is_empty() {
                content.push(json!({"type": "text", "text": t}));
            }
        } else if part["functionCall"].is_object() {
            saw_tool = true;
            call_counter += 1;
            content.push(json!({
                "type": "tool_use",
                "id": format!("toolu_gemini_{call_counter}"),
                "name": part["functionCall"]["name"],
                "input": if part["functionCall"]["args"].is_object() {
                    part["functionCall"]["args"].clone()
                } else {
                    json!({})
                },
            }));
        }
    }
    let finish = resp["candidates"][0]["finishReason"].as_str();
    let stop_reason = if saw_tool {
        "tool_use"
    } else if finish == Some("MAX_TOKENS") {
        "max_tokens"
    } else {
        "end_turn"
    };
    let u = &resp["usageMetadata"];
    json!({
        "id": format!("msg_gemini_{}", resp["responseId"].as_str().unwrap_or("0")),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "usage": {
            "input_tokens": u["promptTokenCount"].as_i64().unwrap_or(0),
            "output_tokens": u["candidatesTokenCount"].as_i64().unwrap_or(0)
                + u["thoughtsTokenCount"].as_i64().unwrap_or(0),
            "cache_read_input_tokens": u["cachedContentTokenCount"].as_i64().unwrap_or(0),
        },
    })
}

pub fn parse_gemini_upstream_final(text: &str) -> Option<Value> {
    let unwrap = |v: Value| -> Value {
        if v["response"].is_object() {
            v["response"].clone()
        } else {
            v
        }
    };
    let trimmed = text.trim_start();
    if !(trimmed.starts_with("data:") || trimmed.starts_with("event:")) {
        return serde_json::from_str::<Value>(text).ok().map(unwrap);
    }
    let mut texts = String::new();
    let mut calls: Vec<Value> = Vec::new();
    let mut finish = Value::Null;
    let mut usage = Value::Null;
    let mut model_version = Value::Null;
    let mut saw_any = false;
    for frame in sse_datas(text) {
        let v = unwrap(frame);
        if !v["candidates"].is_array() && !v["usageMetadata"].is_object() {
            continue;
        }
        saw_any = true;
        for part in v["candidates"][0]["content"]["parts"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(t) = part["text"].as_str() {
                texts.push_str(t);
            } else if part["functionCall"].is_object() {
                calls.push(part.clone());
            }
        }
        if v["candidates"][0]["finishReason"].is_string() {
            finish = v["candidates"][0]["finishReason"].clone();
        }
        if v["usageMetadata"].is_object() {
            usage = v["usageMetadata"].clone();
        }
        if v["modelVersion"].is_string() {
            model_version = v["modelVersion"].clone();
        }
    }
    if !saw_any {
        return None;
    }
    let mut parts = Vec::new();
    if !texts.is_empty() {
        parts.push(json!({"text": texts}));
    }
    parts.extend(calls);
    Some(json!({
        "candidates": [{
            "content": {"role": "model", "parts": parts},
            "finishReason": if finish.is_null() { json!("STOP") } else { finish },
            "index": 0,
        }],
        "usageMetadata": usage,
        "modelVersion": model_version,
    }))
}

pub fn synth_gemini_sse(resp: &Value) -> String {
    let text = gemini_parts_text(&resp["candidates"][0]["content"]["parts"]);
    let mut frames = Vec::new();
    if !text.is_empty() {
        let content_frame = json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": text}]},
                "index": 0,
            }],
            "modelVersion": resp["modelVersion"],
        });
        frames.push(format!("data: {content_frame}\n\n"));
    }
    let mut fin = resp.clone();
    if !text.is_empty() {
        if let Some(parts) = fin["candidates"][0]["content"]["parts"].as_array_mut() {
            parts.retain(|p| p["text"].as_str().is_none());
            if parts.is_empty() {
                parts.push(json!({"text": ""}));
            }
        }
    }
    frames.push(format!("data: {fin}\n\n"));
    frames.concat()
}

pub fn normalize_codex_request(req: &mut Value) {
    let Some(o) = req.as_object_mut() else { return };
    o.insert("store".to_string(), json!(false));
    o.insert("stream".to_string(), json!(true));
    if !o.contains_key("tool_choice") {
        o.insert("tool_choice".to_string(), json!("auto"));
    }
    if !o.contains_key("parallel_tool_calls") {
        o.insert("parallel_tool_calls".to_string(), json!(true));
    }
    o.insert(
        "include".to_string(),
        json!(["reasoning.encrypted_content"]),
    );
    for k in [
        "context_management",
        "max_completion_tokens",
        "max_output_tokens",
        "max_tokens",
        "prompt_cache_retention",
        "safety_identifier",
        "temperature",
        "top_p",
        "truncation",
        "user",
    ] {
        o.remove(k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gemini_req() -> Value {
        json!({
            "model": "gpt-5.5",
            "systemInstruction": {"parts": [{"text": "be terse"}]},
            "contents": [
                {"role": "user", "parts": [{"text": "what is the weather in SF?"}]},
                {"role": "model", "parts": [
                    {"text": "checking"},
                    {"functionCall": {"name": "get_weather", "args": {"city": "SF"}}}
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {"name": "get_weather", "response": {"temp": 18}}}
                ]}
            ],
            "tools": [{"functionDeclarations": [{
                "name": "get_weather",
                "description": "look up weather",
                "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}
            }]}],
            "toolConfig": {"functionCallingConfig": {"mode": "AUTO"}},
            "generationConfig": {
                "temperature": 0.5,
                "topP": 0.9,
                "maxOutputTokens": 1024,
                "stopSequences": ["END"]
            }
        })
    }

    #[test]
    fn gemini_to_anthropic_full() {
        let a = gemini_to_anthropic(&gemini_req());
        assert_eq!(a["model"], "gpt-5.5");
        assert_eq!(a["system"], "be terse");
        assert_eq!(a["max_tokens"], 1024);
        assert_eq!(a["temperature"], 0.5);
        assert_eq!(a["top_p"], 0.9);
        assert_eq!(a["stop_sequences"][0], "END");
        assert_eq!(a["tool_choice"]["type"], "auto");
        assert_eq!(a["tools"][0]["name"], "get_weather");
        assert_eq!(a["tools"][0]["input_schema"]["type"], "object");
        let msgs = a["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"][0]["text"], "what is the weather in SF?");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"][1]["type"], "tool_use");
        assert_eq!(msgs[1]["content"][1]["name"], "get_weather");
        assert_eq!(msgs[1]["content"][1]["input"]["city"], "SF");
        let call_id = msgs[1]["content"][1]["id"].as_str().unwrap();
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][0]["tool_use_id"], call_id);
        assert!(msgs[2]["content"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("18"));
    }

    #[test]
    fn gemini_to_anthropic_defaults() {
        let a = gemini_to_anthropic(&json!({
            "contents": [{"parts": [{"text": "hi"}]}]
        }));
        assert_eq!(a["max_tokens"], 8192);
        assert_eq!(a["messages"][0]["role"], "user");
        assert!(a.get("system").is_none());
        assert!(a.get("tools").is_none());
    }

    #[test]
    fn anthropic_resp_to_gemini_text_and_tools() {
        let resp = json!({
            "id": "msg_1",
            "content": [
                {"type": "text", "text": "PONG"},
                {"type": "tool_use", "id": "t1", "name": "get_weather", "input": {"city": "SF"}}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 3, "cache_read_input_tokens": 4}
        });
        let g = anthropic_response_to_gemini(&resp, "gpt-5.5");
        assert_eq!(g["candidates"][0]["content"]["role"], "model");
        assert_eq!(g["candidates"][0]["content"]["parts"][0]["text"], "PONG");
        assert_eq!(
            g["candidates"][0]["content"]["parts"][1]["functionCall"]["name"],
            "get_weather"
        );
        assert_eq!(g["candidates"][0]["finishReason"], "STOP");
        assert_eq!(g["usageMetadata"]["promptTokenCount"], 14);
        assert_eq!(g["usageMetadata"]["candidatesTokenCount"], 3);
        assert_eq!(g["usageMetadata"]["cachedContentTokenCount"], 4);
        assert_eq!(g["modelVersion"], "gpt-5.5");

        let max = json!({"content": [], "stop_reason": "max_tokens", "usage": {}});
        let g2 = anthropic_response_to_gemini(&max, "m");
        assert_eq!(g2["candidates"][0]["finishReason"], "MAX_TOKENS");
    }

    #[test]
    fn gemini_sse_synth_round_trip() {
        let resp = anthropic_response_to_gemini(
            &json!({
                "content": [{"type": "text", "text": "PONG"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 5, "output_tokens": 1}
            }),
            "gpt-5.5",
        );
        let sse = synth_gemini_sse(&resp);
        assert!(sse.starts_with("data: "));
        assert!(!sse.contains("[DONE]"));
        let frames: Vec<Value> = sse_datas(&sse).collect();
        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[0]["candidates"][0]["content"]["parts"][0]["text"],
            "PONG"
        );
        assert_eq!(frames[1]["candidates"][0]["finishReason"], "STOP");
        assert_eq!(frames[1]["usageMetadata"]["promptTokenCount"], 5);
        assert_eq!(
            assistant_reply_text("gemini", &sse).as_deref(),
            Some("PONG")
        );
    }

    #[test]
    fn anthropic_to_gemini_request_round_trip() {
        let a = json!({
            "model": "gemini-2.5-flash",
            "system": "be terse",
            "max_tokens": 256,
            "temperature": 0.3,
            "messages": [
                {"role": "user", "content": "weather?"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "tu1", "name": "get_weather", "input": {"city": "SF"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "tu1", "content": [{"type": "text", "text": "18C"}]}
                ]}
            ],
            "tools": [{"name": "get_weather", "description": "w", "input_schema": {"type": "object"}}]
        });
        let g = anthropic_to_gemini_request(&a);
        assert_eq!(g["systemInstruction"]["parts"][0]["text"], "be terse");
        assert_eq!(g["generationConfig"]["maxOutputTokens"], 256);
        assert_eq!(g["generationConfig"]["temperature"], 0.3);
        assert_eq!(
            g["tools"][0]["functionDeclarations"][0]["name"],
            "get_weather"
        );
        let c = g["contents"].as_array().unwrap();
        assert_eq!(c[0]["role"], "user");
        assert_eq!(c[0]["parts"][0]["text"], "weather?");
        assert_eq!(c[1]["role"], "model");
        assert_eq!(c[1]["parts"][0]["functionCall"]["name"], "get_weather");
        assert_eq!(c[2]["parts"][0]["functionResponse"]["name"], "get_weather");
        assert_eq!(
            c[2]["parts"][0]["functionResponse"]["response"]["result"],
            "18C"
        );
    }

    #[test]
    fn gemini_response_to_anthropic_basic() {
        let g = json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "PONG"}]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 2, "thoughtsTokenCount": 3}
        });
        let a = gemini_response_to_anthropic(&g, "gemini-2.5-flash");
        assert_eq!(a["role"], "assistant");
        assert_eq!(a["content"][0]["text"], "PONG");
        assert_eq!(a["stop_reason"], "end_turn");
        assert_eq!(a["usage"]["input_tokens"], 10);
        assert_eq!(a["usage"]["output_tokens"], 5);
    }

    #[test]
    fn gemini_upstream_final_unwraps_envelope_and_sse() {
        // non-stream, wrapped in code-assist "response" envelope
        let wrapped = json!({
            "response": {
                "candidates": [{"content": {"role": "model", "parts": [{"text": "hi"}]}, "finishReason": "STOP"}],
                "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1}
            }
        });
        let final_v = parse_gemini_upstream_final(&wrapped.to_string()).unwrap();
        assert_eq!(
            final_v["candidates"][0]["content"]["parts"][0]["text"],
            "hi"
        );
        assert_eq!(final_v["usageMetadata"]["promptTokenCount"], 1);

        // streaming SSE with response-wrapped frames
        let sse = "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"PO\"}]}}]}}\n\n\
                   data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"NG\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":5,\"candidatesTokenCount\":1}}}\n\n";
        let final_sse = parse_gemini_upstream_final(sse).unwrap();
        assert_eq!(
            final_sse["candidates"][0]["content"]["parts"][0]["text"],
            "PONG"
        );
        assert_eq!(final_sse["candidates"][0]["finishReason"], "STOP");
        assert_eq!(final_sse["usageMetadata"]["candidatesTokenCount"], 1);
    }

    #[test]
    fn gemini_last_user_and_reply_text() {
        assert_eq!(
            last_user_text("gemini", &gemini_req()).as_deref(),
            Some("[tool result] {\"temp\":18}")
        );
        let plain = json!({
            "candidates": [{"content": {"role": "model", "parts": [{"text": "hello"}]}}]
        });
        assert_eq!(
            assistant_reply_text("gemini", &plain.to_string()).as_deref(),
            Some("hello")
        );
    }

    use super::*;

    #[test]
    fn chat_to_anthropic_basic() {
        let req = json!({
            "model": "claude-sonnet-4-5",
            "messages": [
                {"role": "system", "content": "be brief"},
                {"role": "system", "content": [{"type": "text", "text": "and kind"}]},
                {"role": "user", "content": [
                    {"type": "text", "text": "hi"},
                    {"type": "image_url", "image_url": {"url": "http://x"}},
                ]},
            ],
            "max_completion_tokens": 512,
            "temperature": 0.5,
            "stop": "END",
            "stream": true,
        });
        let out = openai_chat_to_anthropic(&req);
        assert_eq!(out["system"], "be brief\n\nand kind");
        assert_eq!(out["messages"][0]["role"], "user");
        assert_eq!(out["messages"][0]["content"], "hi");
        assert_eq!(out["max_tokens"], 512);
        assert_eq!(out["temperature"], 0.5);
        assert_eq!(out["stop_sequences"], json!(["END"]));
        assert_eq!(out["stream"], true);
        assert!(out.get("tools").is_none());
    }

    #[test]
    fn chat_to_anthropic_tools_round_trip() {
        let req = json!({
            "model": "gpt-5.1",
            "messages": [
                {"role": "user", "content": "weather?"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"SF\"}"}},
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "sunny"},
            ],
            "tools": [
                {"type": "function", "function": {"name": "get_weather", "description": "d", "parameters": {"type": "object"}}},
            ],
            "tool_choice": {"type": "function", "function": {"name": "get_weather"}},
        });
        let out = openai_chat_to_anthropic(&req);
        let asst = &out["messages"][1];
        assert_eq!(asst["content"][0]["type"], "tool_use");
        assert_eq!(asst["content"][0]["id"], "call_1");
        assert_eq!(asst["content"][0]["input"], json!({"city": "SF"}));
        let result = &out["messages"][2];
        assert_eq!(result["role"], "user");
        assert_eq!(result["content"][0]["type"], "tool_result");
        assert_eq!(result["content"][0]["tool_use_id"], "call_1");
        assert_eq!(result["content"][0]["content"][0]["text"], "sunny");
        assert_eq!(out["tools"][0]["name"], "get_weather");
        assert_eq!(out["tools"][0]["input_schema"], json!({"type": "object"}));
        assert_eq!(
            out["tool_choice"],
            json!({"type": "tool", "name": "get_weather"})
        );
        assert_eq!(out["max_tokens"], 8192);
    }

    #[test]
    fn chat_tool_choice_auto_and_none() {
        let auto = openai_chat_to_anthropic(&json!({"messages": [], "tool_choice": "auto"}));
        assert_eq!(auto["tool_choice"], json!({"type": "auto"}));
        let none = openai_chat_to_anthropic(&json!({"messages": [], "tool_choice": "none"}));
        assert!(none.get("tool_choice").is_none());
    }

    #[test]
    fn responses_to_anthropic() {
        let req = json!({
            "model": "claude-opus-4-8",
            "instructions": "sys",
            "input": [
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "checking"}]},
                {"type": "function_call", "call_id": "c1", "name": "f", "arguments": "{\"a\":1}"},
                {"type": "function_call_output", "call_id": "c1", "output": "42"},
            ],
            "tools": [{"type": "function", "name": "f", "description": "d", "parameters": {"type": "object"}}],
            "max_output_tokens": 100,
            "stream": true,
        });
        let out = openai_responses_to_anthropic(&req);
        assert_eq!(out["system"], "sys");
        assert_eq!(out["messages"][0], json!({"role": "user", "content": "hi"}));
        assert_eq!(out["messages"][1]["role"], "assistant");
        assert_eq!(out["messages"][2]["content"][0]["type"], "tool_use");
        assert_eq!(out["messages"][2]["content"][0]["id"], "c1");
        assert_eq!(out["messages"][2]["content"][0]["input"], json!({"a": 1}));
        assert_eq!(out["messages"][3]["content"][0]["type"], "tool_result");
        assert_eq!(out["messages"][3]["content"][0]["content"][0]["text"], "42");
        assert_eq!(out["tools"][0]["input_schema"], json!({"type": "object"}));
        assert_eq!(out["max_tokens"], 100);
        assert_eq!(out["stream"], true);
    }

    #[test]
    fn responses_to_anthropic_string_input() {
        let out = openai_responses_to_anthropic(&json!({"model": "m", "input": "hello"}));
        assert_eq!(
            out["messages"][0],
            json!({"role": "user", "content": "hello"})
        );
        assert_eq!(out["max_tokens"], 8192);
        assert!(out.get("system").is_none());
    }

    #[test]
    fn anthropic_to_responses() {
        let req = json!({
            "model": "gpt-5.5",
            "system": [{"type": "text", "text": "sys"}],
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "using tool"},
                    {"type": "tool_use", "id": "t1", "name": "f", "input": {"a": 1}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": "ok"}]},
                ]},
            ],
            "tools": [{"name": "f", "description": "d", "input_schema": {"type": "object"}}],
            "max_tokens": 256,
            "thinking": {"type": "adaptive"},
            "output_config": {"effort": "xhigh"},
            "stream": true,
        });
        let out = anthropic_to_openai_responses(&req);
        assert_eq!(out["instructions"], "sys");
        assert_eq!(out["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(out["input"][1]["content"][0]["type"], "output_text");
        assert_eq!(out["input"][2]["type"], "function_call");
        assert_eq!(out["input"][2]["call_id"], "t1");
        assert_eq!(out["input"][2]["arguments"], "{\"a\":1}");
        assert_eq!(out["input"][3]["type"], "function_call_output");
        assert_eq!(out["input"][3]["output"], "ok");
        assert_eq!(out["tools"][0]["type"], "function");
        assert_eq!(out["tools"][0]["parameters"], json!({"type": "object"}));
        assert_eq!(out["tools"][0]["strict"], false);
        assert_eq!(out["max_output_tokens"], 256);
        assert_eq!(out["reasoning"]["effort"], "xhigh");
        assert_eq!(out["stream"], true);
    }

    #[test]
    fn anthropic_to_chat_basic() {
        let req = json!({
            "model": "grok-4",
            "system": "be brief",
            "messages": [
                {"role": "user", "content": "hi"},
            ],
            "max_tokens": 512,
            "temperature": 0.5,
            "top_p": 0.9,
            "stop_sequences": ["END"],
            "stream": true,
            "thinking": {"type": "enabled", "budget_tokens": 4096},
        });
        let out = anthropic_to_openai_chat(&req);
        assert_eq!(out["model"], "grok-4");
        assert_eq!(out["messages"][0], json!({"role": "system", "content": "be brief"}));
        assert_eq!(out["messages"][1], json!({"role": "user", "content": "hi"}));
        assert_eq!(out["max_tokens"], 512);
        assert_eq!(out["temperature"], 0.5);
        assert_eq!(out["top_p"], 0.9);
        assert_eq!(out["stop"], json!(["END"]));
        assert_eq!(out["stream"], true);
        assert!(out.get("thinking").is_none());
        assert!(out.get("tools").is_none());
    }

    #[test]
    fn anthropic_to_chat_tools_round_trip() {
        let req = json!({
            "model": "grok-4",
            "system": [{"type": "text", "text": "sys"}],
            "messages": [
                {"role": "user", "content": "weather?"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "checking"},
                    {"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {"city": "SF"}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1",
                     "content": [{"type": "text", "text": "sunny"}]},
                ]},
            ],
            "tools": [{
                "name": "get_weather",
                "description": "d",
                "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}},
            }],
            "tool_choice": {"type": "tool", "name": "get_weather"},
            "max_tokens": 256,
        });
        let out = anthropic_to_openai_chat(&req);
        assert_eq!(out["messages"][0]["role"], "system");
        assert_eq!(out["messages"][0]["content"], "sys");
        assert_eq!(out["messages"][1]["content"], "weather?");
        let asst = &out["messages"][2];
        assert_eq!(asst["role"], "assistant");
        assert_eq!(asst["content"], "checking");
        assert_eq!(asst["tool_calls"][0]["id"], "call_1");
        assert_eq!(asst["tool_calls"][0]["function"]["name"], "get_weather");
        assert_eq!(asst["tool_calls"][0]["function"]["arguments"], "{\"city\":\"SF\"}");
        let tool = &out["messages"][3];
        assert_eq!(tool["role"], "tool");
        assert_eq!(tool["tool_call_id"], "call_1");
        assert_eq!(tool["content"], "sunny");
        assert_eq!(out["tools"][0]["type"], "function");
        assert_eq!(out["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(
            out["tools"][0]["function"]["parameters"],
            json!({"type": "object", "properties": {"city": {"type": "string"}}})
        );
        assert_eq!(
            out["tool_choice"],
            json!({"type": "function", "function": {"name": "get_weather"}})
        );
        assert_eq!(out["max_tokens"], 256);

        // round-trip through openai_chat_to_anthropic preserves tool shape
        let back = openai_chat_to_anthropic(&out);
        assert_eq!(back["messages"][1]["content"][1]["type"], "tool_use");
        assert_eq!(back["messages"][1]["content"][1]["id"], "call_1");
        assert_eq!(back["messages"][1]["content"][1]["input"], json!({"city": "SF"}));
        assert_eq!(back["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(back["messages"][2]["content"][0]["tool_use_id"], "call_1");
    }

    #[test]
    fn anthropic_to_chat_tool_choice_variants() {
        let auto = anthropic_to_openai_chat(&json!({
            "messages": [], "tool_choice": {"type": "auto"}
        }));
        assert_eq!(auto["tool_choice"], "auto");
        let any = anthropic_to_openai_chat(&json!({
            "messages": [], "tool_choice": {"type": "any"}
        }));
        assert_eq!(any["tool_choice"], "required");
        let none = anthropic_to_openai_chat(&json!({
            "messages": [], "tool_choice": {"type": "none"}
        }));
        assert_eq!(none["tool_choice"], "none");
    }

    fn anthropic_resp() -> Value {
        json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "hi "},
                {"type": "text", "text": "there"},
                {"type": "tool_use", "id": "t1", "name": "f", "input": {"a": 1}},
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 3},
        })
    }

    #[test]
    fn anthropic_resp_to_chat() {
        let out = anthropic_response_to_openai_chat(&anthropic_resp(), "m");
        assert_eq!(out["id"], "chatcmpl-msg_01");
        assert_eq!(out["object"], "chat.completion");
        assert_eq!(out["model"], "m");
        let msg = &out["choices"][0]["message"];
        assert_eq!(msg["content"], "hi there");
        assert_eq!(msg["tool_calls"][0]["id"], "t1");
        assert_eq!(msg["tool_calls"][0]["function"]["arguments"], "{\"a\":1}");
        assert_eq!(out["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(out["usage"]["prompt_tokens"], 10);
        assert_eq!(out["usage"]["completion_tokens"], 5);
        assert_eq!(out["usage"]["total_tokens"], 15);
        assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
    }

    #[test]
    fn chat_resp_to_anthropic() {
        let chat = json!({
            "id": "chatcmpl-abc",
            "object": "chat.completion",
            "model": "grok-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "hi there",
                    "tool_calls": [{
                        "id": "t1",
                        "type": "function",
                        "function": {"name": "f", "arguments": "{\"a\":1}"}
                    }],
                },
                "finish_reason": "tool_calls",
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15,
                "prompt_tokens_details": {"cached_tokens": 3},
            },
        });
        let out = openai_chat_response_to_anthropic(&chat, "grok-4");
        assert_eq!(out["id"], "msg_abc");
        assert_eq!(out["type"], "message");
        assert_eq!(out["role"], "assistant");
        assert_eq!(out["model"], "grok-4");
        assert_eq!(out["content"][0], json!({"type": "text", "text": "hi there"}));
        assert_eq!(out["content"][1]["type"], "tool_use");
        assert_eq!(out["content"][1]["id"], "t1");
        assert_eq!(out["content"][1]["name"], "f");
        assert_eq!(out["content"][1]["input"], json!({"a": 1}));
        assert_eq!(out["stop_reason"], "tool_use");
        assert_eq!(out["usage"]["input_tokens"], 10);
        assert_eq!(out["usage"]["output_tokens"], 5);
        assert_eq!(out["usage"]["cache_read_input_tokens"], 3);

        let stop = json!({
            "id": "chatcmpl-x",
            "choices": [{"message": {"role": "assistant", "content": "done"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1},
        });
        assert_eq!(
            openai_chat_response_to_anthropic(&stop, "m")["stop_reason"],
            "end_turn"
        );
        let len = json!({
            "id": "chatcmpl-y",
            "choices": [{"message": {"role": "assistant", "content": "cut"}, "finish_reason": "length"}],
            "usage": {},
        });
        assert_eq!(
            openai_chat_response_to_anthropic(&len, "m")["stop_reason"],
            "max_tokens"
        );

        // inverse of anthropic_response_to_openai_chat
        let round = openai_chat_response_to_anthropic(
            &anthropic_response_to_openai_chat(&anthropic_resp(), "m"),
            "m",
        );
        assert_eq!(round["content"][0]["text"], "hi there");
        assert_eq!(round["content"][1]["input"], json!({"a": 1}));
        assert_eq!(round["stop_reason"], "tool_use");
    }

    #[test]
    fn anthropic_resp_to_responses() {
        let out = anthropic_response_to_openai_responses(&anthropic_resp(), "m");
        assert_eq!(out["id"], "resp_msg_01");
        assert_eq!(out["status"], "completed");
        assert_eq!(out["output"][0]["type"], "message");
        assert_eq!(out["output"][0]["content"][0]["type"], "output_text");
        assert_eq!(out["output"][0]["content"][0]["text"], "hi ");
        assert_eq!(out["output"][2]["type"], "function_call");
        assert_eq!(out["output"][2]["call_id"], "t1");
        assert_eq!(out["output"][2]["arguments"], "{\"a\":1}");
        assert_eq!(out["usage"]["input_tokens"], 10);
        assert_eq!(out["usage"]["total_tokens"], 15);
        assert_eq!(out["usage"]["input_tokens_details"]["cached_tokens"], 3);
        let mut capped = anthropic_resp();
        capped["stop_reason"] = json!("max_tokens");
        assert_eq!(
            anthropic_response_to_openai_responses(&capped, "m")["status"],
            "incomplete"
        );
    }

    fn responses_resp() -> Value {
        json!({
            "id": "r1",
            "object": "response",
            "status": "completed",
            "output": [
                {"type": "reasoning", "id": "rs1", "summary": []},
                {"type": "message", "id": "m1", "role": "assistant", "status": "completed",
                 "content": [{"type": "output_text", "text": "hello", "annotations": []}]},
                {"type": "function_call", "id": "fc1", "call_id": "c1", "name": "f", "arguments": "{\"a\":1}"},
            ],
            "usage": {"input_tokens": 7, "output_tokens": 2, "input_tokens_details": {"cached_tokens": 4}},
        })
    }

    #[test]
    fn responses_to_anthropic_resp() {
        let out = responses_final_to_anthropic(&responses_resp(), "m");
        assert_eq!(out["id"], "msg_r1");
        assert_eq!(out["type"], "message");
        assert_eq!(out["content"][0], json!({"type": "text", "text": "hello"}));
        assert_eq!(out["content"][1]["type"], "tool_use");
        assert_eq!(out["content"][1]["id"], "c1");
        assert_eq!(out["content"][1]["input"], json!({"a": 1}));
        assert_eq!(out["stop_reason"], "tool_use");
        assert_eq!(out["usage"]["input_tokens"], 7);
        assert_eq!(out["usage"]["cache_read_input_tokens"], 4);
        let mut inc = responses_resp();
        inc["status"] = json!("incomplete");
        assert_eq!(
            responses_final_to_anthropic(&inc, "m")["stop_reason"],
            "max_tokens"
        );
        let mut plain = responses_resp();
        plain["output"].as_array_mut().unwrap().pop();
        assert_eq!(
            responses_final_to_anthropic(&plain, "m")["stop_reason"],
            "end_turn"
        );
    }

    #[test]
    fn responses_to_chat_resp() {
        let out = responses_final_to_openai_chat(&responses_resp(), "m");
        assert_eq!(out["id"], "chatcmpl-r1");
        let msg = &out["choices"][0]["message"];
        assert_eq!(msg["content"], "hello");
        assert_eq!(msg["tool_calls"][0]["id"], "c1");
        assert_eq!(msg["tool_calls"][0]["function"]["arguments"], "{\"a\":1}");
        assert_eq!(out["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(out["usage"]["prompt_tokens"], 7);
        assert_eq!(out["usage"]["total_tokens"], 9);
        assert_eq!(out["usage"]["prompt_tokens_details"]["cached_tokens"], 4);
    }

    #[test]
    fn anthropic_sse_reassembly() {
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hel\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"f\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"a\\\":\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"1}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":25}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let m = parse_anthropic_sse_to_message(sse).unwrap();
        assert_eq!(m["content"][0]["text"], "hello");
        assert_eq!(m["content"][1]["type"], "tool_use");
        assert_eq!(m["content"][1]["input"], json!({"a": 1}));
        assert_eq!(m["stop_reason"], "tool_use");
        assert_eq!(m["usage"]["input_tokens"], 10);
        assert_eq!(m["usage"]["output_tokens"], 25);
        assert!(parse_anthropic_sse_to_message("data: {\"type\":\"ping\"}\n\n").is_none());
    }

    #[test]
    fn responses_sse_final() {
        let sse = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\",\"status\":\"in_progress\"}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"status\":\"completed\",\"output\":[]}}\n\n",
        );
        let r = parse_responses_sse_final(sse).unwrap();
        assert_eq!(r["id"], "r1");
        assert_eq!(r["status"], "completed");
        assert!(parse_responses_sse_final("data: {\"type\":\"response.created\"}\n\n").is_none());
    }

    #[test]
    fn chat_sse_synth() {
        let chat = anthropic_response_to_openai_chat(&anthropic_resp(), "m");
        let sse = synth_openai_chat_sse(&chat);
        let chunks: Vec<Value> = sse
            .lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .filter(|d| *d != "[DONE]")
            .map(|d| serde_json::from_str(d).unwrap())
            .collect();
        assert_eq!(chunks[0]["choices"][0]["delta"]["role"], "assistant");
        assert_eq!(chunks[0]["object"], "chat.completion.chunk");
        assert_eq!(chunks[1]["choices"][0]["delta"]["content"], "hi there");
        assert_eq!(
            chunks[2]["choices"][0]["delta"]["tool_calls"][0]["index"],
            0
        );
        assert_eq!(
            chunks[2]["choices"][0]["delta"]["tool_calls"][0]["id"],
            "t1"
        );
        let last = chunks.last().unwrap();
        assert_eq!(last["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(last["usage"]["total_tokens"], 15);
        assert!(sse.ends_with("data: [DONE]\n\n"));
    }

    #[test]
    fn chat_sse_parse_and_anthropic_resynth() {
        // chat SSE deltas → final chat → anthropic → anthropic SSE
        let chat = json!({
            "id": "chatcmpl-xyz",
            "object": "chat.completion",
            "model": "grok-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "hello",
                    "tool_calls": [{
                        "id": "t1",
                        "type": "function",
                        "function": {"name": "f", "arguments": "{\"a\":1}"}
                    }],
                },
                "finish_reason": "tool_calls",
            }],
            "usage": {
                "prompt_tokens": 8,
                "completion_tokens": 4,
                "total_tokens": 12,
                "prompt_tokens_details": {"cached_tokens": 2},
            },
        });
        let sse = synth_openai_chat_sse(&chat);
        let parsed = parse_openai_chat_sse_final(&sse).unwrap();
        assert_eq!(parsed["id"], "chatcmpl-xyz");
        assert_eq!(parsed["choices"][0]["message"]["content"], "hello");
        assert_eq!(
            parsed["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"a\":1}"
        );
        assert_eq!(parsed["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(parsed["usage"]["prompt_tokens"], 8);

        let anth = openai_chat_response_to_anthropic(&parsed, "grok-4");
        assert_eq!(anth["content"][0]["text"], "hello");
        assert_eq!(anth["content"][1]["input"], json!({"a": 1}));
        assert_eq!(anth["stop_reason"], "tool_use");
        assert_eq!(anth["usage"]["input_tokens"], 8);
        assert_eq!(anth["usage"]["cache_read_input_tokens"], 2);

        let anth_sse = synth_anthropic_sse(&anth);
        assert!(anth_sse.starts_with("event: message_start\n"));
        assert!(anth_sse.contains("event: content_block_delta\n"));
        assert!(anth_sse.contains("event: message_stop\n"));
        let reassembled = parse_anthropic_sse_to_message(&anth_sse).unwrap();
        assert_eq!(reassembled["content"][0]["text"], "hello");
        assert_eq!(reassembled["content"][1]["input"], json!({"a": 1}));
        assert_eq!(reassembled["stop_reason"], "tool_use");
        assert_eq!(reassembled["usage"]["output_tokens"], 4);

        // text-only streaming deltas
        let text_sse = concat!(
            "data: {\"id\":\"chatcmpl-t\",\"object\":\"chat.completion.chunk\",\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl-t\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"PO\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl-t\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"NG\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chatcmpl-t\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1,\"total_tokens\":4}}\n\n",
            "data: [DONE]\n\n",
        );
        let t = parse_openai_chat_sse_final(text_sse).unwrap();
        assert_eq!(t["choices"][0]["message"]["content"], "PONG");
        assert_eq!(t["choices"][0]["finish_reason"], "stop");
        assert_eq!(t["usage"]["prompt_tokens"], 3);
        let a = openai_chat_response_to_anthropic(&t, "m");
        assert_eq!(a["content"][0]["text"], "PONG");
        assert_eq!(a["stop_reason"], "end_turn");
        assert!(parse_openai_chat_sse_final("data: {\"type\":\"ping\"}\n\n").is_none());
    }

    #[test]
    fn responses_sse_synth() {
        let sse = synth_openai_responses_sse(&responses_resp());
        assert!(sse.starts_with("event: response.created\n"));
        assert!(sse.contains("event: response.output_item.added\n"));
        assert!(sse.contains("event: response.output_item.done\n"));
        assert!(sse.contains("\"type\":\"function_call\""));
        assert!(sse.contains("event: response.output_text.delta\n"));
        assert!(sse.contains("event: response.output_text.done\n"));
        assert!(sse.contains("event: response.completed\n"));
        let fin = parse_responses_sse_final(&sse).unwrap();
        assert_eq!(fin, responses_resp());
    }

    #[test]
    fn anthropic_sse_synth() {
        let sse = synth_anthropic_sse(&anthropic_resp());
        assert!(sse.starts_with("event: message_start\n"));
        assert!(sse.contains("event: content_block_start\n"));
        assert!(sse.contains("event: message_stop\n"));
        let m = parse_anthropic_sse_to_message(&sse).unwrap();
        assert_eq!(m["content"][0]["text"], "hi ");
        assert_eq!(m["content"][2]["input"], json!({"a": 1}));
        assert_eq!(m["stop_reason"], "tool_use");
        assert_eq!(m["usage"]["input_tokens"], 10);
        assert_eq!(m["usage"]["output_tokens"], 5);
    }

    #[test]
    fn codex_normalize() {
        let mut req = json!({
            "model": "gpt-5.1-codex",
            "input": [],
            "temperature": 0.7,
            "top_p": 0.9,
            "max_output_tokens": 100,
            "max_tokens": 100,
            "max_completion_tokens": 100,
            "truncation": "auto",
            "user": "u",
            "safety_identifier": "s",
            "prompt_cache_retention": "24h",
            "context_management": {},
            "reasoning": {"effort": "high"},
            "text": {"verbosity": "low"},
            "prompt_cache_key": "k",
            "service_tier": "flex",
            "tool_choice": "none",
        });
        normalize_codex_request(&mut req);
        assert_eq!(req["store"], false);
        assert_eq!(req["stream"], true);
        assert_eq!(req["tool_choice"], "none");
        assert_eq!(req["parallel_tool_calls"], true);
        assert_eq!(req["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(req["reasoning"]["effort"], "high");
        assert_eq!(req["text"]["verbosity"], "low");
        assert_eq!(req["prompt_cache_key"], "k");
        assert_eq!(req["service_tier"], "flex");
        for k in [
            "temperature",
            "top_p",
            "max_output_tokens",
            "max_tokens",
            "max_completion_tokens",
            "truncation",
            "user",
            "safety_identifier",
            "prompt_cache_retention",
            "context_management",
        ] {
            assert!(req.get(k).is_none(), "{k} should be removed");
        }
    }

    #[test]
    fn codex_normalize_defaults() {
        let mut req = json!({"model": "m", "input": []});
        normalize_codex_request(&mut req);
        assert_eq!(req["tool_choice"], "auto");
        assert_eq!(req["parallel_tool_calls"], true);
    }

    #[test]
    fn codex_normalize_preserves_ultra_effort() {
        let mut req = json!({
            "model": "gpt-5.6-sol",
            "input": [],
            "reasoning": {"effort": "ultra"},
        });
        normalize_codex_request(&mut req);
        assert_eq!(req["reasoning"]["effort"], "ultra");
    }

    #[test]
    fn last_user_text_anthropic() {
        let req = json!({"messages": [
            {"role": "user", "content": "first"},
            {"role": "assistant", "content": "reply"},
            {"role": "user", "content": [
                {"type": "text", "text": "part1"},
                {"type": "text", "text": "part2"},
            ]},
        ]});
        assert_eq!(
            last_user_text("anthropic", &req),
            Some("part1\npart2".into())
        );
        let long = "x".repeat(500);
        let tool = json!({"messages": [
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "f", "input": {}}]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1",
                 "content": [{"type": "text", "text": long}]},
            ]},
        ]});
        let got = last_user_text("anthropic", &tool).unwrap();
        assert!(got.starts_with("[tool result] xxx"));
        assert_eq!(got.chars().count(), "[tool result] ".chars().count() + 200);
        assert_eq!(last_user_text("anthropic", &json!({"messages": []})), None);
    }

    #[test]
    fn last_user_text_openai_chat() {
        let req = json!({"messages": [
            {"role": "system", "content": "s"},
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "hi"},
            {"role": "user", "content": [{"type": "text", "text": "again"}]},
        ]});
        assert_eq!(last_user_text("openai-chat", &req), Some("again".into()));
        let tool = json!({"messages": [
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": null},
            {"role": "tool", "tool_call_id": "c1", "content": "result body"},
        ]});
        assert_eq!(
            last_user_text("openai-chat", &tool),
            Some("[tool result] result body".into())
        );
        assert_eq!(last_user_text("openai-chat", &json!({})), None);
    }

    #[test]
    fn last_user_text_openai_responses() {
        let req = json!({"input": [
            {"type": "message", "role": "user",
             "content": [{"type": "input_text", "text": "one"}]},
            {"type": "message", "role": "assistant",
             "content": [{"type": "output_text", "text": "r"}]},
            {"type": "message", "role": "user",
             "content": [{"type": "input_text", "text": "two"}]},
        ]});
        assert_eq!(last_user_text("openai-responses", &req), Some("two".into()));
        let tool = json!({"input": [
            {"type": "message", "role": "user",
             "content": [{"type": "input_text", "text": "q"}]},
            {"type": "function_call", "call_id": "c1", "name": "f", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "c1", "output": "tool says hi"},
        ]});
        assert_eq!(
            last_user_text("openai-responses", &tool),
            Some("[tool result] tool says hi".into())
        );
        assert_eq!(
            last_user_text("openai-responses", &json!({"input": "raw"})),
            Some("raw".into())
        );
        assert_eq!(last_user_text("mystery", &json!({})), None);
    }

    #[test]
    fn assistant_reply_anthropic_plain_and_sse() {
        let plain = json!({
            "id": "msg_01", "type": "message", "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "hmm"},
                {"type": "text", "text": "hello "},
                {"type": "text", "text": "world"},
            ],
            "stop_reason": "end_turn",
        });
        assert_eq!(
            assistant_reply_text("anthropic", &plain.to_string()),
            Some("hello world".into())
        );
        let sse = synth_anthropic_sse(&anthropic_resp());
        assert_eq!(
            assistant_reply_text("anthropic", &sse),
            Some("hi there".into())
        );
        assert_eq!(assistant_reply_text("anthropic", "not json"), None);
    }

    #[test]
    fn assistant_reply_openai_chat_plain_and_sse() {
        let plain =
            json!({"choices": [{"message": {"role": "assistant", "content": "chat reply"}}]});
        assert_eq!(
            assistant_reply_text("openai-chat", &plain.to_string()),
            Some("chat reply".into())
        );
        let sse = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"str\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"eamed\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        assert_eq!(
            assistant_reply_text("openai-chat", sse),
            Some("streamed".into())
        );
        assert_eq!(assistant_reply_text("openai-chat", "data: {}\n\n"), None);
    }

    #[test]
    fn assistant_reply_openai_responses_plain_and_sse() {
        assert_eq!(
            assistant_reply_text("openai-responses", &responses_resp().to_string()),
            Some("hello".into())
        );
        let sse = synth_openai_responses_sse(&responses_resp());
        assert_eq!(
            assistant_reply_text("openai-responses", &sse),
            Some("hello".into())
        );
        assert_eq!(
            assistant_reply_text("openai-responses", "data: {\"type\":\"ping\"}\n\n"),
            None
        );
    }
}
