//! Grok web-billing via grok.com gRPC-web RPC (`GetGrokCreditsConfig`).
//!
//! Ported from CodexBar's `GrokWebBillingFetcher` — parses SuperGrok weekly credit
//! utilization and reset time from the protobuf response. Does not perform HTTP.

use std::borrow::Cow;

/// Default endpoint used by grok.com's usage page.
pub const GROK_CREDITS_ENDPOINT: &str =
    "https://grok.com/grok_api_v2.GrokBuildBilling/GetGrokCreditsConfig";

/// Empty gRPC-web frame body (flags=0, length=0) sent for the no-arg RPC.
pub const GROK_CREDITS_REQUEST_BODY: &[u8] = &[0x00, 0x00, 0x00, 0x00, 0x00];

#[derive(Debug, Clone, PartialEq)]
pub struct GrokWebBillingSnapshot {
    /// Used percent of SuperGrok credits (0–100).
    pub used_percent: f64,
    /// Unix epoch seconds when the credit window resets, if known.
    pub resets_at_s: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrokWebBillingError {
    EmptyResponse,
    ParseFailed,
    RpcFailed { status: i32, message: String },
}

impl std::fmt::Display for GrokWebBillingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyResponse => write!(f, "Grok web billing returned no protobuf payload"),
            Self::ParseFailed => write!(f, "Could not parse Grok web billing usage"),
            Self::RpcFailed { status, message } => {
                write!(
                    f,
                    "Grok web billing RPC failed with status {status}: {message}"
                )
            }
        }
    }
}

impl std::error::Error for GrokWebBillingError {}

/// Window label derived from how far out the reset is (matches CodexBar labeling).
pub fn window_label(resets_at_s: Option<i64>, now_s: i64) -> &'static str {
    let Some(reset) = resets_at_s else {
        return "7d";
    };
    let secs = (reset - now_s).max(0) as f64;
    if secs <= 3600.0 {
        return "7d";
    }
    let days = (secs / 86400.0).round() as i64;
    if (4..=12).contains(&days) {
        "7d"
    } else if (20..=45).contains(&days) {
        "30d"
    } else {
        "7d"
    }
}

/// Parse a gRPC-web response body into a billing snapshot.
///
/// Accepts framed gRPC-web data frames and unframed protobuf payloads (as grok
/// sometimes returns). `now_s` is used to prefer future reset timestamps.
pub fn parse_grpc_web_response(
    data: &[u8],
    now_s: i64,
) -> Result<GrokWebBillingSnapshot, GrokWebBillingError> {
    validate_grpc_web_trailers(data)?;

    let mut payloads = grpc_web_data_frames(data);
    if payloads.is_empty() && looks_like_protobuf_payload(data) {
        payloads = vec![data.to_vec()];
    }
    if payloads.is_empty() {
        return Err(GrokWebBillingError::EmptyResponse);
    }

    let mut scan = ProtobufScan::default();
    for payload in &payloads {
        scan.merge(scan_protobuf(payload, 0, &[], 0).0);
    }

    let parsed_percent = scan
        .fixed32_fields
        .iter()
        .filter(|f| {
            f.path.last() == Some(&1) && f.value.is_finite() && (0.0..=100.0).contains(&f.value)
        })
        .min_by(|a, b| a.path.len().cmp(&b.path.len()).then(a.order.cmp(&b.order)))
        .map(|f| f.value as f64);

    let reset_fields: Vec<(Vec<u64>, i64)> = scan
        .varint_fields
        .iter()
        .filter_map(|f| {
            let raw = f.value;
            if (1_700_000_000..=2_100_000_000).contains(&raw) {
                Some((f.path.clone(), raw as i64))
            } else {
                None
            }
        })
        .collect();

    let future_resets: Vec<(Vec<u64>, i64)> = reset_fields
        .into_iter()
        .filter(|(_, ts)| *ts > now_s)
        .collect();

    let preferred = future_resets
        .iter()
        .filter(|(path, _)| path.as_slice() == [1, 5, 1])
        .map(|(_, ts)| *ts)
        .min();
    let reset = preferred.or_else(|| future_resets.iter().map(|(_, ts)| *ts).min());

    let has_usage_period = scan.varint_fields.iter().any(|f| {
        f.path.starts_with(&[1, 6])
            || (f.path.as_slice() == [1, 8, 1] && (f.value == 1 || f.value == 2))
    });
    let no_usage_yet = parsed_percent.is_none()
        && scan.fixed32_fields.is_empty()
        && reset.is_some()
        && has_usage_period;

    let percent = match parsed_percent {
        Some(p) => p,
        None if no_usage_yet => 0.0,
        None => return Err(GrokWebBillingError::ParseFailed),
    };

    Ok(GrokWebBillingSnapshot {
        used_percent: percent,
        resets_at_s: reset,
    })
}

/// Validate gRPC status from response headers (keys lowercased).
pub fn validate_grpc_status_headers(
    headers: impl IntoIterator<Item = (impl AsRef<str>, impl AsRef<str>)>,
) -> Result<(), GrokWebBillingError> {
    let mut status: Option<i32> = None;
    let mut message = String::new();
    for (k, v) in headers {
        let key = k.as_ref().trim().to_ascii_lowercase();
        if key == "grpc-status" {
            status = v.as_ref().trim().parse().ok();
        } else if key == "grpc-message" {
            message = percent_decode(v.as_ref().trim()).into_owned();
        }
    }
    if let Some(s) = status {
        if s != 0 {
            return Err(GrokWebBillingError::RpcFailed { status: s, message });
        }
    }
    Ok(())
}

pub fn looks_like_protobuf_payload(data: &[u8]) -> bool {
    let Some(&first) = data.first() else {
        return false;
    };
    let field_number = first >> 3;
    let wire_type = first & 0x07;
    field_number > 0 && matches!(wire_type, 0 | 1 | 2 | 5)
}

pub fn grpc_web_data_frames(data: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut index = 0;
    while index < data.len() {
        if index + 5 > data.len() {
            return Vec::new();
        }
        let flags = data[index];
        let length = u32::from_be_bytes([
            data[index + 1],
            data[index + 2],
            data[index + 3],
            data[index + 4],
        ]) as usize;
        let start = index + 5;
        let end = start.saturating_add(length);
        if end > data.len() {
            return Vec::new();
        }
        if flags & 0x80 == 0 {
            frames.push(data[start..end].to_vec());
        }
        index = end;
    }
    frames
}

fn validate_grpc_web_trailers(data: &[u8]) -> Result<(), GrokWebBillingError> {
    let fields = grpc_web_trailer_fields(data);
    if let Some(raw) = fields.get("grpc-status") {
        if let Ok(status) = raw.parse::<i32>() {
            if status != 0 {
                let message = fields.get("grpc-message").cloned().unwrap_or_default();
                return Err(GrokWebBillingError::RpcFailed { status, message });
            }
        }
    }
    Ok(())
}

pub fn grpc_web_trailer_fields(data: &[u8]) -> std::collections::HashMap<String, String> {
    let mut fields = std::collections::HashMap::new();
    let mut index = 0;
    while index + 5 <= data.len() {
        let flags = data[index];
        let length = u32::from_be_bytes([
            data[index + 1],
            data[index + 2],
            data[index + 3],
            data[index + 4],
        ]) as usize;
        let start = index + 5;
        let end = start.saturating_add(length);
        if end > data.len() {
            break;
        }
        if flags & 0x80 != 0 {
            if let Ok(text) = std::str::from_utf8(&data[start..end]) {
                for line in text.lines() {
                    if line.is_empty() {
                        continue;
                    }
                    let Some((key, value)) = line.split_once(':') else {
                        continue;
                    };
                    let key = key.trim().to_ascii_lowercase();
                    let value = percent_decode(value.trim()).into_owned();
                    fields.insert(key, value);
                }
            }
        }
        index = end;
    }
    fields
}

fn percent_decode(s: &str) -> Cow<'_, str> {
    if !s.contains('%') {
        return Cow::Borrowed(s);
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Cow::Owned(String::from_utf8_lossy(&out).into_owned())
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[derive(Default)]
struct ProtobufScan {
    fixed32_fields: Vec<Fixed32Field>,
    varint_fields: Vec<VarintField>,
}

struct Fixed32Field {
    path: Vec<u64>,
    value: f32,
    order: usize,
}

struct VarintField {
    path: Vec<u64>,
    value: u64,
}

impl ProtobufScan {
    fn merge(&mut self, other: ProtobufScan) {
        self.fixed32_fields.extend(other.fixed32_fields);
        self.varint_fields.extend(other.varint_fields);
    }
}

fn scan_protobuf(data: &[u8], depth: usize, path: &[u64], order: usize) -> (ProtobufScan, usize) {
    let mut scan = ProtobufScan::default();
    let mut index = 0;
    let mut next_order = order;

    while index < data.len() {
        let field_start = index;
        let Some(key) = read_varint(data, &mut index) else {
            index = field_start + 1;
            continue;
        };
        if key == 0 {
            index = field_start + 1;
            continue;
        }
        let field_number = key >> 3;
        let wire_type = key & 0x07;
        let mut field_path = path.to_vec();
        field_path.push(field_number);

        match wire_type {
            0 => {
                if let Some(value) = read_varint(data, &mut index) {
                    scan.varint_fields.push(VarintField {
                        path: field_path,
                        value,
                    });
                } else {
                    index = field_start + 1;
                }
            }
            1 => {
                if index + 8 > data.len() {
                    return (scan, next_order);
                }
                index += 8;
            }
            2 => {
                let Some(length) = read_varint(data, &mut index) else {
                    index = field_start + 1;
                    continue;
                };
                if length as usize > data.len().saturating_sub(index) {
                    index = field_start + 1;
                    continue;
                }
                let start = index;
                let end = index + length as usize;
                if depth < 4 {
                    let (nested, order) =
                        scan_protobuf(&data[start..end], depth + 1, &field_path, next_order);
                    scan.merge(nested);
                    next_order = order;
                }
                index = end;
            }
            5 => {
                if index + 4 > data.len() {
                    return (scan, next_order);
                }
                let bits = u32::from_le_bytes([
                    data[index],
                    data[index + 1],
                    data[index + 2],
                    data[index + 3],
                ]);
                scan.fixed32_fields.push(Fixed32Field {
                    path: field_path,
                    value: f32::from_bits(bits),
                    order: next_order,
                });
                next_order += 1;
                index += 4;
            }
            _ => {
                index = field_start + 1;
            }
        }
    }

    (scan, next_order)
}

fn read_varint(bytes: &[u8], index: &mut usize) -> Option<u64> {
    let mut value: u64 = 0;
    let mut shift: u64 = 0;
    while *index < bytes.len() && shift < 64 {
        let byte = bytes[*index];
        *index += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(mut value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let mut byte = (value & 0x7F) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            bytes.push(byte);
            if value == 0 {
                break;
            }
        }
        bytes
    }

    fn protobuf_payload(used_percent: f32, reset_epoch: u64) -> Vec<u8> {
        let mut data = Vec::new();
        data.push(0x0D); // field 1, fixed32
        data.extend_from_slice(&used_percent.to_bits().to_le_bytes());
        data.push(0x10); // field 2, varint
        data.extend(varint(reset_epoch));
        data
    }

    fn grpc_frame(payload: &[u8], flags: u8) -> Vec<u8> {
        let mut data = vec![flags];
        data.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        data.extend_from_slice(payload);
        data
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn parses_grok_grpc_web_billing_frame() {
        let reset = 1_800_000_000u64;
        let payload = protobuf_payload(42.5, reset);
        let data = grpc_frame(&payload, 0x00);

        let snap = parse_grpc_web_response(&data, 1_799_000_000).unwrap();
        assert!((snap.used_percent - 42.5).abs() < 1e-5);
        assert_eq!(snap.resets_at_s, Some(reset as i64));
    }

    #[test]
    fn rejects_empty_truncated_and_garbage_grpc_web_responses() {
        let cases: &[(&str, &[u8], GrokWebBillingError)] = &[
            ("empty", &[], GrokWebBillingError::EmptyResponse),
            (
                "header shorter than five bytes",
                &[0x00, 0x00, 0x00, 0x00],
                GrokWebBillingError::EmptyResponse,
            ),
            (
                "declared payload is truncated",
                &[0x00, 0x00, 0x00, 0x00, 0x04, 0x0D, 0x00],
                GrokWebBillingError::EmptyResponse,
            ),
            (
                "garbage wire type",
                &[0xFF, 0xFF, 0xFF],
                GrokWebBillingError::EmptyResponse,
            ),
            (
                "protobuf-shaped garbage",
                &[0x08, 0x80],
                GrokWebBillingError::ParseFailed,
            ),
        ];

        for (name, body, expected) in cases {
            assert_eq!(
                parse_grpc_web_response(body, 1_800_000_000),
                Err(expected.clone()),
                "{name}"
            );
        }
    }

    #[test]
    fn parses_unframed_grok_billing_protobuf_payload() {
        // Captured fixture from CodexBar GrokWebBillingFetcherTests.
        let data = hex(
            "0a3f0d7f6a9c3f12001a002206088097f3d0062a060880b191d2063a07080215a9389b3f3a07080115d6ea183c421208011206088097f3d0061a060880b191d206",
        );
        let snap = parse_grpc_web_response(&data, 1_780_000_000).unwrap();
        assert!((snap.used_percent - 1.222000002861023).abs() < 1e-6);
        assert_eq!(snap.resets_at_s, Some(1_782_864_000));
    }

    #[test]
    fn parses_unframed_zero_percent_payload() {
        let reset = 1_800_000_000u64;
        let payload = protobuf_payload(0.0, reset);
        assert!(grpc_web_data_frames(&payload).is_empty());

        let snap = parse_grpc_web_response(&payload, 1_799_000_000).unwrap();
        assert_eq!(snap.used_percent, 0.0);
        assert_eq!(snap.resets_at_s, Some(reset as i64));
    }

    #[test]
    fn does_not_treat_grpc_frame_prefix_as_raw_protobuf() {
        assert!(!looks_like_protobuf_payload(&[0, 0, 0, 0, 10]));
    }

    #[test]
    fn ignores_grpc_web_trailer_frames() {
        let payload = protobuf_payload(12.25, 1_800_000_001);
        let trailer = b"grpc-status: 0\r\n";
        let data = {
            let mut d = grpc_frame(&payload, 0x00);
            d.extend(grpc_frame(trailer, 0x80));
            d
        };
        let frames = grpc_web_data_frames(&data);
        assert_eq!(frames, vec![payload]);
    }

    #[test]
    fn rejects_reset_only_billing() {
        let mut payload = vec![0x10]; // field 2, varint
        payload.extend(varint(1_800_000_001));
        let data = grpc_frame(&payload, 0x00);
        assert_eq!(
            parse_grpc_web_response(&data, 1_700_000_000),
            Err(GrokWebBillingError::ParseFailed)
        );
    }

    #[test]
    fn parses_no_usage_yet_as_zero_percent() {
        // Captured fixture from CodexBar (no fixed32 usage percent, has usage period).
        let data: Vec<u8> = vec![
            0x00, 0x00, 0x00, 0x00, 0x37, 0x0A, 0x35, 0x12, 0x00, 0x1A, 0x00, 0x22, 0x06, 0x08,
            0x80, 0xDA, 0xCF, 0xCF, 0x06, 0x2A, 0x06, 0x08, 0x80, 0x97, 0xF3, 0xD0, 0x06, 0x32,
            0x09, 0x0A, 0x05, 0x08, 0xEA, 0x0F, 0x10, 0x04, 0x12, 0x00, 0x32, 0x09, 0x0A, 0x05,
            0x08, 0xEA, 0x0F, 0x10, 0x03, 0x12, 0x00, 0x32, 0x09, 0x0A, 0x05, 0x08, 0xEA, 0x0F,
            0x10, 0x02, 0x12, 0x00, 0x80, 0x00, 0x00, 0x00, 0x0F, 0x67, 0x72, 0x70, 0x63, 0x2D,
            0x73, 0x74, 0x61, 0x74, 0x75, 0x73, 0x3A, 0x30, 0x0D, 0x0A,
        ];
        let snap = parse_grpc_web_response(&data, 1_768_000_000).unwrap();
        assert_eq!(snap.used_percent, 0.0);
        assert_eq!(snap.resets_at_s, Some(1_780_272_000));
    }

    #[test]
    fn parses_omitted_zero_percent_with_current_billing_period() {
        let data: Vec<u8> = vec![
            0x00, 0x00, 0x00, 0x00, 0x2A, 0x0A, 0x28, 0x12, 0x00, 0x1A, 0x00, 0x22, 0x06, 0x08,
            0x80, 0x97, 0xF3, 0xD0, 0x06, 0x2A, 0x06, 0x08, 0x80, 0xB1, 0x91, 0xD2, 0x06, 0x42,
            0x12, 0x08, 0x01, 0x12, 0x06, 0x08, 0x80, 0x97, 0xF3, 0xD0, 0x06, 0x1A, 0x06, 0x08,
            0x80, 0xB1, 0x91, 0xD2, 0x06, 0x80, 0x00, 0x00, 0x00, 0x0F, 0x67, 0x72, 0x70, 0x63,
            0x2D, 0x73, 0x74, 0x61, 0x74, 0x75, 0x73, 0x3A, 0x30, 0x0D, 0x0A,
        ];
        let snap = parse_grpc_web_response(&data, 1_781_000_000).unwrap();
        assert_eq!(snap.used_percent, 0.0);
        assert_eq!(snap.resets_at_s, Some(1_782_864_000));
    }

    #[test]
    fn uses_billing_field_one_instead_of_earlier_unrelated_float() {
        let mut payload = Vec::new();
        payload.push(0x4D); // field 9, fixed32
        payload.extend_from_slice(&7.0f32.to_bits().to_le_bytes());
        payload.push(0x0D); // field 1, fixed32
        payload.extend_from_slice(&42.0f32.to_bits().to_le_bytes());
        payload.push(0x10); // field 2, varint
        payload.extend(varint(1_800_000_001));

        let snap = parse_grpc_web_response(&grpc_frame(&payload, 0x00), 1_700_000_000).unwrap();
        assert_eq!(snap.used_percent, 42.0);
    }

    #[test]
    fn chooses_future_billing_end_instead_of_recent_start() {
        let recent_start = 1_800_000_000u64;
        let billing_end = 1_802_592_000u64;
        let mut payload = Vec::new();
        payload.push(0x0D);
        payload.extend_from_slice(&33.0f32.to_bits().to_le_bytes());
        payload.push(0x10);
        payload.extend(varint(recent_start));
        payload.push(0x18);
        payload.extend(varint(billing_end));

        let snap =
            parse_grpc_web_response(&grpc_frame(&payload, 0x00), (recent_start + 1800) as i64)
                .unwrap();
        assert_eq!(snap.resets_at_s, Some(billing_end as i64));
    }

    #[test]
    fn trailer_rpc_failure_is_detected() {
        let body = grpc_frame(
            b"grpc-status: 16\r\ngrpc-message: token%20expired\r\n",
            0x80,
        );
        let fields = grpc_web_trailer_fields(&body);
        assert_eq!(fields.get("grpc-status").map(String::as_str), Some("16"));
        assert_eq!(
            fields.get("grpc-message").map(String::as_str),
            Some("token expired")
        );
        assert!(matches!(
            parse_grpc_web_response(&body, 0),
            Err(GrokWebBillingError::RpcFailed { status: 16, .. })
        ));
    }

    #[test]
    fn window_label_weekly_and_monthly() {
        let now = 1_800_000_000i64;
        assert_eq!(window_label(Some(now + 6 * 86400), now), "7d");
        assert_eq!(window_label(Some(now + 30 * 86400), now), "30d");
        assert_eq!(window_label(None, now), "7d");
    }

    #[test]
    fn validate_grpc_headers() {
        assert!(validate_grpc_status_headers([("grpc-status", "0")]).is_ok());
        assert!(matches!(
            validate_grpc_status_headers([
                ("grpc-status", "16"),
                ("grpc-message", "Invalid%20bearer%20token.")
            ]),
            Err(GrokWebBillingError::RpcFailed { status: 16, .. })
        ));
    }
}
