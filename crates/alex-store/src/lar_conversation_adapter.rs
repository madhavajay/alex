//! Bounded provider-wire adapters for the conversation catalog.
//!
//! The adapters are deliberately outside `alex-lar`: the container core knows
//! only canonical records and raw ranges. This parser retains lexical JSON
//! spans and decodes only bounded object keys and semantic discriminator
//! strings. It never reserializes a provider body.

use anyhow::{bail, Context, Result};

use crate::{LarConversationEntryKind, LarConversationRole};

pub(crate) const MAX_ADAPTER_BODY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_JSON_DEPTH: usize = 128;
const MAX_JSON_NODES: usize = 200_000;
const MAX_CONVERSATION_ENTRIES: usize = 8_192;
const MAX_KEY_BYTES: usize = 1_024;
const MAX_DISCRIMINATOR_BYTES: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WireDirection {
    Request,
    Response,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdaptedEntry {
    pub role: LarConversationRole,
    pub kind: LarConversationEntryKind,
    pub name: Option<String>,
    pub tool_call_id: Option<String>,
    pub byte_offset: u64,
    pub byte_length: u64,
}

#[derive(Clone, Copy, Debug)]
struct AdapterLimits {
    max_body_bytes: usize,
    max_depth: usize,
    max_nodes: usize,
    max_entries: usize,
    max_key_bytes: usize,
    max_discriminator_bytes: usize,
}

impl Default for AdapterLimits {
    fn default() -> Self {
        Self {
            max_body_bytes: MAX_ADAPTER_BODY_BYTES as usize,
            max_depth: MAX_JSON_DEPTH,
            max_nodes: MAX_JSON_NODES,
            max_entries: MAX_CONVERSATION_ENTRIES,
            max_key_bytes: MAX_KEY_BYTES,
            max_discriminator_bytes: MAX_DISCRIMINATOR_BYTES,
        }
    }
}

pub(crate) fn adapt_wire_body(
    source_format: &str,
    direction: WireDirection,
    body: &[u8],
) -> Result<Vec<AdaptedEntry>> {
    adapt_wire_body_with_limits(source_format, direction, body, AdapterLimits::default())
}

fn adapt_wire_body_with_limits(
    source_format: &str,
    direction: WireDirection,
    body: &[u8],
    limits: AdapterLimits,
) -> Result<Vec<AdaptedEntry>> {
    if body.len() > limits.max_body_bytes {
        bail!("provider body exceeds bounded conversation adapter limit");
    }
    let document = JsonDocument::parse(body, limits)?;
    let mut entries = match (canonical_format(source_format), direction) {
        (Some(ProviderFormat::Anthropic), WireDirection::Request) => anthropic_request(&document)?,
        (Some(ProviderFormat::Anthropic), WireDirection::Response) => {
            anthropic_response(&document)?
        }
        (Some(ProviderFormat::OpenAiChat), WireDirection::Request) => {
            openai_chat_request(&document)?
        }
        (Some(ProviderFormat::OpenAiChat), WireDirection::Response) => {
            openai_chat_response(&document)?
        }
        (Some(ProviderFormat::OpenAiResponses), WireDirection::Request) => {
            openai_responses_request(&document)?
        }
        (Some(ProviderFormat::OpenAiResponses), WireDirection::Response) => {
            openai_responses_response(&document)?
        }
        (Some(ProviderFormat::Gemini), WireDirection::Request) => gemini_request(&document)?,
        (Some(ProviderFormat::Gemini), WireDirection::Response) => gemini_response(&document)?,
        (None, _) => bail!("unknown provider wire format"),
    };
    if entries.is_empty() {
        bail!("provider body contains no bounded conversation entries");
    }
    if entries.len() > limits.max_entries {
        bail!("provider body exceeds conversation entry limit");
    }
    entries.sort_by_key(|entry| entry.byte_offset);
    Ok(entries)
}

#[derive(Clone, Copy)]
enum ProviderFormat {
    Anthropic,
    OpenAiChat,
    OpenAiResponses,
    Gemini,
}

fn canonical_format(value: &str) -> Option<ProviderFormat> {
    match value {
        "anthropic" | "anthropic-messages" | "anthropic-messages-v1" => {
            Some(ProviderFormat::Anthropic)
        }
        "openai" | "openai-chat" | "openai-chat-v1" => Some(ProviderFormat::OpenAiChat),
        "openai-responses" | "openai-responses-v1" => Some(ProviderFormat::OpenAiResponses),
        "gemini" | "gemini-generate" | "gemini-generate-v1" => Some(ProviderFormat::Gemini),
        _ => None,
    }
}

fn anthropic_request(document: &JsonDocument<'_>) -> Result<Vec<AdaptedEntry>> {
    let root = document.root_object()?;
    let mut output = Vec::new();
    if let Some(system) = document.member(root, "system") {
        output.push(document.entry(
            system,
            LarConversationRole::System,
            LarConversationEntryKind::Message,
            None,
            None,
        )?);
    }
    let messages = document
        .member(root, "messages")
        .context("Anthropic request has no messages array")?;
    for message in document.array(messages)? {
        output.push(message_entry(
            document,
            *message,
            MessageDialect::Anthropic,
        )?);
    }
    Ok(output)
}

fn anthropic_response(document: &JsonDocument<'_>) -> Result<Vec<AdaptedEntry>> {
    let root = document.root_object()?;
    if document.string_member(root, "role")?.as_deref() != Some("assistant")
        || document.member(root, "content").is_none()
    {
        bail!("Anthropic response is not a message object");
    }
    Ok(vec![message_entry(
        document,
        root,
        MessageDialect::Anthropic,
    )?])
}

fn openai_chat_request(document: &JsonDocument<'_>) -> Result<Vec<AdaptedEntry>> {
    let root = document.root_object()?;
    let messages = document
        .member(root, "messages")
        .context("OpenAI chat request has no messages array")?;
    document
        .array(messages)?
        .iter()
        .map(|message| message_entry(document, *message, MessageDialect::OpenAi))
        .collect()
}

fn openai_chat_response(document: &JsonDocument<'_>) -> Result<Vec<AdaptedEntry>> {
    let root = document.root_object()?;
    let choices = document
        .member(root, "choices")
        .context("OpenAI chat response has no choices")?;
    let mut output = Vec::new();
    for choice in document.array(choices)? {
        let message = document
            .member(*choice, "message")
            .context("OpenAI chat choice has no message")?;
        output.push(message_entry(document, message, MessageDialect::OpenAi)?);
    }
    Ok(output)
}

fn openai_responses_request(document: &JsonDocument<'_>) -> Result<Vec<AdaptedEntry>> {
    let root = document.root_object()?;
    let mut output = Vec::new();
    if let Some(instructions) = document.member(root, "instructions") {
        output.push(document.entry(
            instructions,
            LarConversationRole::System,
            LarConversationEntryKind::Message,
            None,
            None,
        )?);
    }
    let input = document
        .member(root, "input")
        .context("OpenAI Responses request has no input")?;
    match document.kind(input) {
        NodeKind::String => output.push(document.entry(
            input,
            LarConversationRole::User,
            LarConversationEntryKind::Message,
            None,
            None,
        )?),
        NodeKind::Array(items) => {
            for item in items {
                output.push(responses_item(document, *item)?);
            }
        }
        _ => bail!("OpenAI Responses input is neither a string nor an array"),
    }
    Ok(output)
}

fn openai_responses_response(document: &JsonDocument<'_>) -> Result<Vec<AdaptedEntry>> {
    let root = document.root_object()?;
    let output = document
        .member(root, "output")
        .context("OpenAI Responses response has no output")?;
    document
        .array(output)?
        .iter()
        .map(|item| responses_item(document, *item))
        .collect()
}

fn responses_item(document: &JsonDocument<'_>, node: usize) -> Result<AdaptedEntry> {
    let item_type = document.string_member(node, "type")?;
    let (role, kind) = match item_type.as_deref() {
        Some("function_call") | Some("computer_call") => (
            LarConversationRole::Assistant,
            LarConversationEntryKind::ToolCall,
        ),
        Some("function_call_output") | Some("computer_call_output") => (
            LarConversationRole::Tool,
            LarConversationEntryKind::ToolResult,
        ),
        _ => (
            role(document.string_member(node, "role")?.as_deref())?,
            LarConversationEntryKind::Message,
        ),
    };
    document.entry(
        node,
        role,
        kind,
        document.string_member(node, "name")?,
        document
            .string_member(node, "call_id")?
            .or(document.string_member(node, "tool_call_id")?),
    )
}

fn gemini_request(document: &JsonDocument<'_>) -> Result<Vec<AdaptedEntry>> {
    let root = document.root_object()?;
    let mut output = Vec::new();
    if let Some(system) = document
        .member(root, "systemInstruction")
        .or_else(|| document.member(root, "system_instruction"))
    {
        output.push(document.entry(
            system,
            LarConversationRole::System,
            LarConversationEntryKind::Message,
            None,
            None,
        )?);
    }
    let contents = document
        .member(root, "contents")
        .context("Gemini request has no contents")?;
    for content in document.array(contents)? {
        output.push(message_entry(document, *content, MessageDialect::Gemini)?);
    }
    Ok(output)
}

fn gemini_response(document: &JsonDocument<'_>) -> Result<Vec<AdaptedEntry>> {
    let root = document.root_object()?;
    let candidates = document
        .member(root, "candidates")
        .context("Gemini response has no candidates")?;
    let mut output = Vec::new();
    for candidate in document.array(candidates)? {
        let content = document
            .member(*candidate, "content")
            .context("Gemini candidate has no content")?;
        output.push(message_entry(document, content, MessageDialect::Gemini)?);
    }
    Ok(output)
}

#[derive(Clone, Copy)]
enum MessageDialect {
    Anthropic,
    OpenAi,
    Gemini,
}

fn message_entry(
    document: &JsonDocument<'_>,
    node: usize,
    dialect: MessageDialect,
) -> Result<AdaptedEntry> {
    let role_value = document.string_member(node, "role")?;
    let mut role = match dialect {
        MessageDialect::Gemini if role_value.as_deref() == Some("model") => {
            LarConversationRole::Assistant
        }
        _ => role(role_value.as_deref())?,
    };
    let mut kind = LarConversationEntryKind::Message;
    let mut tool_call_id = document.string_member(node, "tool_call_id")?;
    let name = document.string_member(node, "name")?;
    if role == LarConversationRole::Tool {
        kind = LarConversationEntryKind::ToolResult;
    } else if document.member(node, "tool_calls").is_some()
        || document.member(node, "functionCall").is_some()
        || document.member(node, "function_call").is_some()
    {
        kind = LarConversationEntryKind::ToolCall;
    }
    if matches!(dialect, MessageDialect::Gemini) {
        if let Some(parts) = document.member(node, "parts") {
            for part in document.array(parts)? {
                if let Some(call) = document.member(*part, "functionCall") {
                    kind = LarConversationEntryKind::ToolCall;
                    tool_call_id = document
                        .string_member(call, "id")?
                        .or(document.string_member(call, "name")?);
                }
                if let Some(response) = document.member(*part, "functionResponse") {
                    role = LarConversationRole::Tool;
                    kind = LarConversationEntryKind::ToolResult;
                    tool_call_id = document
                        .string_member(response, "id")?
                        .or(document.string_member(response, "name")?);
                    break;
                }
            }
        }
    }
    if matches!(dialect, MessageDialect::Anthropic) {
        if let Some(content) = document.member(node, "content") {
            if let Ok(items) = document.array(content) {
                for item in items {
                    match document.string_member(*item, "type")?.as_deref() {
                        Some("tool_result") => {
                            role = LarConversationRole::Tool;
                            kind = LarConversationEntryKind::ToolResult;
                            tool_call_id = document.string_member(*item, "tool_use_id")?;
                            break;
                        }
                        Some("tool_use") => {
                            kind = LarConversationEntryKind::ToolCall;
                            tool_call_id = document.string_member(*item, "id")?;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    document.entry(node, role, kind, name, tool_call_id)
}

fn role(value: Option<&str>) -> Result<LarConversationRole> {
    match value {
        Some("system" | "developer") => Ok(LarConversationRole::System),
        Some("user") => Ok(LarConversationRole::User),
        Some("assistant" | "model") => Ok(LarConversationRole::Assistant),
        Some("tool" | "function") => Ok(LarConversationRole::Tool),
        _ => bail!("provider message has an unknown or missing role"),
    }
}

#[derive(Debug)]
struct JsonDocument<'a> {
    bytes: &'a [u8],
    nodes: Vec<Node>,
    root: usize,
    limits: AdapterLimits,
}

#[derive(Debug)]
struct Node {
    start: usize,
    end: usize,
    kind: NodeKind,
}

#[derive(Debug)]
enum NodeKind {
    Object(Vec<Member>),
    Array(Vec<usize>),
    String,
    Scalar,
}

#[derive(Debug)]
struct Member {
    key: String,
    value: usize,
}

struct JsonParser<'a> {
    document: JsonDocument<'a>,
    cursor: usize,
}

impl<'a> JsonDocument<'a> {
    fn parse(bytes: &'a [u8], limits: AdapterLimits) -> Result<Self> {
        let mut parser = JsonParser {
            document: Self {
                bytes,
                nodes: Vec::new(),
                root: 0,
                limits,
            },
            cursor: 0,
        };
        let root = parser.parse_value(0)?;
        parser.skip_whitespace();
        if parser.cursor != bytes.len() {
            bail!("provider JSON has trailing bytes");
        }
        parser.document.root = root;
        Ok(parser.document)
    }

    fn root_object(&self) -> Result<usize> {
        if matches!(self.kind(self.root), NodeKind::Object(_)) {
            Ok(self.root)
        } else {
            bail!("provider JSON root is not an object")
        }
    }

    fn kind(&self, node: usize) -> &NodeKind {
        &self.nodes[node].kind
    }

    fn member(&self, node: usize, key: &str) -> Option<usize> {
        match self.kind(node) {
            NodeKind::Object(members) => members
                .iter()
                .find(|member| member.key == key)
                .map(|member| member.value),
            _ => None,
        }
    }

    fn array(&self, node: usize) -> Result<&[usize]> {
        match self.kind(node) {
            NodeKind::Array(items) => Ok(items),
            _ => bail!("provider JSON field is not an array"),
        }
    }

    fn string(&self, node: usize) -> Result<String> {
        let value = &self.nodes[node];
        if !matches!(value.kind, NodeKind::String) {
            bail!("provider JSON discriminator is not a string");
        }
        if value.end.saturating_sub(value.start) > self.limits.max_discriminator_bytes {
            bail!("provider JSON discriminator exceeds its bound");
        }
        serde_json::from_slice(&self.bytes[value.start..value.end])
            .context("decoding provider JSON discriminator")
    }

    fn string_member(&self, node: usize, key: &str) -> Result<Option<String>> {
        self.member(node, key)
            .map(|value| self.string(value))
            .transpose()
    }

    fn entry(
        &self,
        node: usize,
        role: LarConversationRole,
        kind: LarConversationEntryKind,
        name: Option<String>,
        tool_call_id: Option<String>,
    ) -> Result<AdaptedEntry> {
        let node = &self.nodes[node];
        Ok(AdaptedEntry {
            role,
            kind,
            name,
            tool_call_id,
            byte_offset: node.start as u64,
            byte_length: node.end.saturating_sub(node.start) as u64,
        })
    }
}

impl JsonParser<'_> {
    fn parse_value(&mut self, depth: usize) -> Result<usize> {
        if depth > self.document.limits.max_depth {
            bail!("provider JSON exceeds nesting limit");
        }
        if self.document.nodes.len() >= self.document.limits.max_nodes {
            bail!("provider JSON exceeds node limit");
        }
        self.skip_whitespace();
        let start = self.cursor;
        let kind = match self.peek().context("unexpected end of provider JSON")? {
            b'{' => self.parse_object(depth)?,
            b'[' => self.parse_array(depth)?,
            b'"' => {
                self.scan_string()?;
                NodeKind::String
            }
            b't' => {
                self.literal(b"true")?;
                NodeKind::Scalar
            }
            b'f' => {
                self.literal(b"false")?;
                NodeKind::Scalar
            }
            b'n' => {
                self.literal(b"null")?;
                NodeKind::Scalar
            }
            b'-' | b'0'..=b'9' => {
                self.scan_number()?;
                NodeKind::Scalar
            }
            _ => bail!("invalid provider JSON value"),
        };
        if self.document.nodes.len() >= self.document.limits.max_nodes {
            bail!("provider JSON exceeds node limit");
        }
        let id = self.document.nodes.len();
        self.document.nodes.push(Node {
            start,
            end: self.cursor,
            kind,
        });
        Ok(id)
    }

    fn parse_object(&mut self, depth: usize) -> Result<NodeKind> {
        self.cursor += 1;
        self.skip_whitespace();
        let mut members = Vec::new();
        if self.consume(b'}') {
            return Ok(NodeKind::Object(members));
        }
        loop {
            self.skip_whitespace();
            let key_start = self.cursor;
            self.scan_string()?;
            let key_end = self.cursor;
            if key_end.saturating_sub(key_start) > self.document.limits.max_key_bytes {
                bail!("provider JSON object key exceeds its bound");
            }
            let key: String = serde_json::from_slice(&self.document.bytes[key_start..key_end])
                .context("decoding provider JSON object key")?;
            if members.iter().any(|member: &Member| member.key == key) {
                bail!("provider JSON object contains a duplicate key");
            }
            self.skip_whitespace();
            self.expect(b':')?;
            let value = self.parse_value(depth + 1)?;
            members.push(Member { key, value });
            self.skip_whitespace();
            if self.consume(b'}') {
                break;
            }
            self.expect(b',')?;
        }
        Ok(NodeKind::Object(members))
    }

    fn parse_array(&mut self, depth: usize) -> Result<NodeKind> {
        self.cursor += 1;
        self.skip_whitespace();
        let mut items = Vec::new();
        if self.consume(b']') {
            return Ok(NodeKind::Array(items));
        }
        loop {
            items.push(self.parse_value(depth + 1)?);
            self.skip_whitespace();
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
        }
        Ok(NodeKind::Array(items))
    }

    fn scan_string(&mut self) -> Result<()> {
        self.expect(b'"')?;
        let content_start = self.cursor;
        while let Some(byte) = self.peek() {
            match byte {
                b'"' => {
                    std::str::from_utf8(&self.document.bytes[content_start..self.cursor])
                        .context("provider JSON string contains invalid UTF-8")?;
                    self.cursor += 1;
                    return Ok(());
                }
                b'\\' => {
                    self.cursor += 1;
                    match self.peek().context("unterminated provider JSON escape")? {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
                            self.cursor += 1;
                        }
                        b'u' => {
                            self.cursor += 1;
                            let scalar = self.take_unicode_escape()?;
                            if (0xd800..=0xdbff).contains(&scalar) {
                                if !self.consume(b'\\') || !self.consume(b'u') {
                                    bail!("unpaired high surrogate in provider JSON string");
                                }
                                let low = self.take_unicode_escape()?;
                                if !(0xdc00..=0xdfff).contains(&low) {
                                    bail!("invalid low surrogate in provider JSON string");
                                }
                            } else if (0xdc00..=0xdfff).contains(&scalar) {
                                bail!("unpaired low surrogate in provider JSON string");
                            }
                        }
                        _ => bail!("invalid provider JSON escape"),
                    }
                }
                0x00..=0x1f => bail!("unescaped control byte in provider JSON string"),
                _ => self.cursor += 1,
            }
        }
        bail!("unterminated provider JSON string")
    }

    fn take_unicode_escape(&mut self) -> Result<u16> {
        let start = self.cursor;
        for _ in 0..4 {
            if !self.peek().is_some_and(|value| value.is_ascii_hexdigit()) {
                bail!("invalid provider JSON unicode escape");
            }
            self.cursor += 1;
        }
        let digits = std::str::from_utf8(&self.document.bytes[start..self.cursor])?;
        u16::from_str_radix(digits, 16).context("invalid provider JSON unicode escape")
    }

    fn scan_number(&mut self) -> Result<()> {
        let start = self.cursor;
        if self.consume(b'-') && self.peek().is_none() {
            bail!("invalid provider JSON number");
        }
        if self.consume(b'0') {
            if self.peek().is_some_and(|value| value.is_ascii_digit()) {
                bail!("invalid provider JSON leading zero");
            }
        } else {
            self.take_digits(true)?;
        }
        if self.consume(b'.') {
            self.take_digits(true)?;
        }
        if self
            .peek()
            .is_some_and(|value| matches!(value, b'e' | b'E'))
        {
            self.cursor += 1;
            if self
                .peek()
                .is_some_and(|value| matches!(value, b'+' | b'-'))
            {
                self.cursor += 1;
            }
            self.take_digits(true)?;
        }
        serde_json::from_slice::<serde_json::Number>(&self.document.bytes[start..self.cursor])
            .context("validating provider JSON number")?;
        Ok(())
    }

    fn take_digits(&mut self, required: bool) -> Result<()> {
        let start = self.cursor;
        while self.peek().is_some_and(|value| value.is_ascii_digit()) {
            self.cursor += 1;
        }
        if required && self.cursor == start {
            bail!("invalid provider JSON number");
        }
        Ok(())
    }

    fn literal(&mut self, value: &[u8]) -> Result<()> {
        if self
            .document
            .bytes
            .get(self.cursor..self.cursor + value.len())
            != Some(value)
        {
            bail!("invalid provider JSON literal");
        }
        self.cursor += value.len();
        Ok(())
    }

    fn skip_whitespace(&mut self) {
        while self
            .peek()
            .is_some_and(|value| matches!(value, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.cursor += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.document.bytes.get(self.cursor).copied()
    }

    fn consume(&mut self, value: u8) -> bool {
        if self.peek() == Some(value) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, value: u8) -> Result<()> {
        if self.consume(value) {
            Ok(())
        } else {
            bail!("invalid provider JSON punctuation")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slices<'a>(body: &'a [u8], entries: &[AdaptedEntry]) -> Vec<&'a [u8]> {
        entries
            .iter()
            .map(|entry| {
                &body[entry.byte_offset as usize..(entry.byte_offset + entry.byte_length) as usize]
            })
            .collect()
    }

    #[test]
    fn anthropic_spans_preserve_escapes_and_nested_objects_exactly() {
        let body = br#" {"syst\u0065m" : "keep \\n escaped", "mess\u0061ges" : [ { "role":"u\u0073er", "content":[{"type":"text","text":"{nested}"}] }, {"role":"assistant","content":[{"type":"tool_use","id":"call-1","input":{"x":[1,2]}}]} ] } "#;
        let entries = adapt_wire_body("anthropic", WireDirection::Request, body).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].role, LarConversationRole::System);
        assert_eq!(entries[2].kind, LarConversationEntryKind::ToolCall);
        assert_eq!(entries[2].tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(
            slices(body, &entries),
            [
                br#""keep \\n escaped""#.as_slice(),
                br#"{ "role":"u\u0073er", "content":[{"type":"text","text":"{nested}"}] }"#,
                br#"{"role":"assistant","content":[{"type":"tool_use","id":"call-1","input":{"x":[1,2]}}]}"#,
            ]
        );
    }

    #[test]
    fn provider_shapes_extract_exact_message_objects() {
        let cases: &[(&str, WireDirection, &[u8], &[&[u8]])] = &[
            (
                "openai-chat",
                WireDirection::Request,
                br#"{"messages":[ {"role":"system","content":"s"}, {"role":"tool","tool_call_id":"c","content":"r"}]}"#,
                &[
                    br#"{"role":"system","content":"s"}"#,
                    br#"{"role":"tool","tool_call_id":"c","content":"r"}"#,
                ],
            ),
            (
                "openai-responses",
                WireDirection::Request,
                br#"{"instructions":"be exact","input":[{"role":"user","content":[{"type":"input_text","text":"hi"}]},{"type":"function_call_output","call_id":"c","output":"ok"}]}"#,
                &[
                    br#""be exact""#,
                    br#"{"role":"user","content":[{"type":"input_text","text":"hi"}]}"#,
                    br#"{"type":"function_call_output","call_id":"c","output":"ok"}"#,
                ],
            ),
            (
                "gemini",
                WireDirection::Request,
                br#"{"systemInstruction":{"parts":[{"text":"s"}]},"contents":[{"role":"user","parts":[{"text":"hi"}]},{"role":"model","parts":[{"text":"yo"}]}]}"#,
                &[
                    br#"{"parts":[{"text":"s"}]}"#,
                    br#"{"role":"user","parts":[{"text":"hi"}]}"#,
                    br#"{"role":"model","parts":[{"text":"yo"}]}"#,
                ],
            ),
        ];
        for (format, direction, body, expected) in cases {
            let entries = adapt_wire_body(format, *direction, body).unwrap();
            assert_eq!(slices(body, &entries), *expected, "format {format}");
        }
    }

    #[test]
    fn provider_response_shapes_retain_exact_nested_spans() {
        let cases: &[(&str, &[u8], &[&[u8]])] = &[
            (
                "anthropic",
                br#"{"id":"m","role":"assistant","content":[{"type":"text","text":"hi"}]}"#,
                &[br#"{"id":"m","role":"assistant","content":[{"type":"text","text":"hi"}]}"#],
            ),
            (
                "openai-chat",
                br#"{"choices":[{"index":0,"message":{"role":"assistant","content":"hi"}}]}"#,
                &[br#"{"role":"assistant","content":"hi"}"#],
            ),
            (
                "openai-responses",
                br#"{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi"}]},{"type":"function_call","call_id":"c","name":"f","arguments":"{}"}]}"#,
                &[
                    br#"{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi"}]}"#,
                    br#"{"type":"function_call","call_id":"c","name":"f","arguments":"{}"}"#,
                ],
            ),
            (
                "gemini",
                br#"{"candidates":[{"content":{"role":"model","parts":[{"text":"hi"}]}}]}"#,
                &[br#"{"role":"model","parts":[{"text":"hi"}]}"#],
            ),
        ];
        for (format, body, expected) in cases {
            let entries = adapt_wire_body(format, WireDirection::Response, body).unwrap();
            assert_eq!(slices(body, &entries), *expected, "format {format}");
        }
    }

    #[test]
    fn malformed_unknown_and_resource_exhaustion_fall_back_to_caller() {
        assert!(adapt_wire_body(
            "anthropic",
            WireDirection::Request,
            br#"{"messages":[{"role":"user","content":"unterminated}] }"#
        )
        .is_err());
        assert!(adapt_wire_body(
            "anthropic",
            WireDirection::Request,
            b"{\"messages\":[{\"role\":\"user\",\"content\":\"\xff\"}]}"
        )
        .is_err());
        assert!(adapt_wire_body(
            "anthropic",
            WireDirection::Request,
            br#"{"messages":[{"role":"user","content":"\uD800"}]}"#
        )
        .is_err());
        assert!(adapt_wire_body(
            "anthropic",
            WireDirection::Request,
            br#"{"messages":[],"messages":[]}"#
        )
        .is_err());
        assert!(adapt_wire_body(
            "future-provider",
            WireDirection::Request,
            br#"{"messages":[]}"#
        )
        .is_err());
        let nested = br#"{"messages":[[[[{"role":"user"}]]]]}"#;
        assert!(adapt_wire_body_with_limits(
            "anthropic",
            WireDirection::Request,
            nested,
            AdapterLimits {
                max_depth: 2,
                ..AdapterLimits::default()
            }
        )
        .is_err());
        assert!(adapt_wire_body_with_limits(
            "anthropic",
            WireDirection::Request,
            br#"{"messages":[]}"#,
            AdapterLimits {
                max_body_bytes: 4,
                ..AdapterLimits::default()
            }
        )
        .is_err());
        assert!(adapt_wire_body_with_limits(
            "anthropic",
            WireDirection::Request,
            br#"{"messages":[{"role":"user"}]}"#,
            AdapterLimits {
                max_nodes: 2,
                ..AdapterLimits::default()
            }
        )
        .is_err());
    }
}
